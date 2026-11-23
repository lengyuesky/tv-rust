//! 飞牛播放器协议适配。兼容版本字段仅用于协商，不表示本项目是 fnOS。
use crate::{
    access, auth,
    catalog::{self, Episode, Film},
    core::*,
    fntv_identity as identity, fntv_playback as native_playback, net, playback, probe,
};
use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::Method,
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
use tokio::time::{Instant, timeout_at};

/// 带来源菜单的直链交付所报的存储类型。实测 1.13.0 对 9001 之外未登记的类型按通用
/// 直连处理：不显示任何网盘品牌，保留画质菜单。
const DIRECT_MENU_STORAGE_TYPE: u64 = 9000;

fn ok(v: Value) -> Response {
    let mut response = Json(json!({"code":0,"msg":"","data":v})).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
pub fn library(kind: &str) -> String {
    guid(&["library", kind])
}
pub fn season(film: &str) -> String {
    guid(&["season", film, "1"])
}
async fn source_season_info(app: &App, id: &str) -> Result<Option<Value>> {
    let view: Option<(String, String, String)> =
        sqlx::query_as("SELECT film_id,line_name,episode_id FROM fntv_source_versions WHERE id=?")
            .bind(id)
            .fetch_optional(&app.db)
            .await?;
    Ok(view.map(|(film_id,line_name,episode_id)|json!({"film_id":film_id,"line_name":line_name,"episode_id":episode_id})))
}
pub async fn handle(State(app): State<App>, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let response = inner(&app, req)
        .await
        .unwrap_or_else(IntoResponse::into_response);
    // 仅记录路径种类，不输出令牌、密码、请求体或外部媒体地址。
    let category = path.split('/').take(6).collect::<Vec<_>>().join("/");
    tracing::info!(method=%method,path=%category,status=response.status().as_u16(),"飞牛请求");
    response
}
async fn resolve_film(app: &App, id: &str) -> Result<Film> {
    if let Ok(f) = catalog::film(app, id).await {
        return Ok(f);
    }
    if let Ok(e) = catalog::episode(app, id).await {
        return catalog::film(app, &e.film_id).await;
    }
    let work = if let Some((work, _)) = identity::episode_work(app, id).await? {
        Some(work)
    } else {
        identity::find(app, id).await?
    };
    if let Some(work) = work {
        return identity::members(app, &work.group_key)
            .await?
            .into_iter()
            .next()
            .ok_or_else(Error::missing);
    }
    if let Some(view) = source_season_info(app, id).await? {
        return catalog::film(app, &text(&view, "film_id")).await;
    }
    let films: Vec<Film> = sqlx::query_as("SELECT * FROM films")
        .fetch_all(&app.db)
        .await?;
    films
        .into_iter()
        .find(|f| season(&f.id) == id)
        .ok_or_else(Error::missing)
}
async fn authorized_film(app: &App, who: &Identity, id: &str) -> Result<Film> {
    let work = if let Some((work, _)) = identity::episode_work(app, id).await? {
        Some(work)
    } else {
        identity::find(app, id).await?
    };
    if let Some(work) = work {
        return identity::preferred(app, who, &work).await;
    }
    let f = resolve_film(app, id).await?;
    access::require_source(app, &who.user.id, &f.source_id).await?;
    Ok(f)
}
async fn resolve_episode(app: &App, who: &Identity, id: &str) -> Result<Episode> {
    if let Ok(e) = catalog::episode(app, id).await {
        return Ok(catalog::available(app, &who.user.id, &e.id).await?.0);
    }
    if let Some((work, number)) = identity::episode_work(app, id).await? {
        let f = identity::preferred(app, who, &work).await?;
        catalog::detail(app, &who.user.id, &f.id, false).await?;
        let eps = work_episodes(app, who, &f).await?;
        let matched: Vec<_> = eps
            .into_iter()
            .filter(|e| catalog::episode_number(&e.title) == Some(number))
            .collect();
        if matched.len() != 1 {
            return Err(Error::bad("当前来源无法确认该单集，请重新选择播放源"));
        }
        return Ok(catalog::available(app, &who.user.id, &matched[0].id)
            .await?
            .0);
    }
    let f = authorized_film(app, who, id).await?;
    let f = catalog::detail(app, &who.user.id, &f.id, false).await?;
    let eps = selected_line_episodes(app, who, id, &f.id).await?;
    let prior: Option<String> =
        sqlx::query_scalar("SELECT episode_id FROM progress WHERE user_id=? AND film_id=?")
            .bind(&who.user.id)
            .bind(&f.id)
            .fetch_optional(&app.db)
            .await?;
    let reference = source_season_info(app, id)
        .await?
        .map(|v| text(&v, "episode_id"));
    if let Some(reference) = reference {
        let e = catalog::available(app, &who.user.id, &reference).await?.0;
        if e.film_id != f.id || !eps.iter().any(|candidate| candidate.id == e.id) {
            return Err(Error::bad("该播放版本已更新，请重新选择来源"));
        }
        return Ok(e);
    }
    let e = eps
        .iter()
        .find(|e| prior.as_deref() == Some(&e.id))
        .or_else(|| eps.first())
        .cloned()
        .ok_or_else(|| Error::bad("该影片没有可识别的播放地址"))?;
    Ok(catalog::available(app, &who.user.id, &e.id).await?.0)
}
async fn selected_line_episodes(
    app: &App,
    who: &Identity,
    id: &str,
    film: &str,
) -> Result<Vec<Episode>> {
    let eps = catalog::episodes(app, film).await?;
    let explicit = source_season_info(app, id)
        .await?
        .map(|v| text(&v, "line_name"));
    if explicit
        .as_ref()
        .is_some_and(|line| !eps.iter().any(|e| &e.line_name == line))
    {
        return Err(Error::bad("该播放线路已更新，请重新选择来源"));
    }
    let active: Option<String> = sqlx::query_scalar("SELECT e.line_name FROM plays p JOIN episodes e ON e.id=p.episode_id WHERE p.auth_hash=? AND p.film_id=? AND p.ended=0 AND p.expires_at>? ORDER BY p.seq DESC LIMIT 1")
        .bind(&who.auth_hash).bind(film).bind(now()).fetch_optional(&app.db).await?;
    let previous: Option<String> = sqlx::query_scalar("SELECT e.line_name FROM progress p JOIN episodes e ON e.id=p.episode_id WHERE p.user_id=? AND p.film_id=?")
        .bind(&who.user.id).bind(film).fetch_optional(&app.db).await?;
    let line = explicit
        .or(active)
        .or(previous)
        .filter(|line| eps.iter().any(|e| &e.line_name == line))
        .or_else(|| default_line(&eps));
    Ok(eps
        .into_iter()
        .filter(|e| Some(&e.line_name) == line.as_ref())
        .collect())
}

/// 地址或线路名看起来是可直接交付的媒体，而不是需要网页解析的播放页。
pub(crate) fn media_like(e: &Episode) -> bool {
    e.line_name.to_ascii_lowercase().contains("m3u8")
        || net::valid_url(&e.url).is_ok_and(|url| {
            let path = url.path().to_ascii_lowercase();
            [
                ".m3u8", ".mp4", ".mkv", ".flv", ".ts", ".mov", ".webm", ".avi",
            ]
            .iter()
            .any(|extension| path.ends_with(extension))
        })
}

/// 没有观看记录时的默认线路。多数采集源的第一条线路是网页播放器，
/// 优先落在已识别出视频或地址形态为媒体的线路上，全部无法判断时沿用第一条。
fn default_line(eps: &[Episode]) -> Option<String> {
    let playable = |e: &Episode| match probe::last_known(e) {
        Some(info) => {
            probe::require_available(&info).is_ok()
                && (info.kind == "hls" || probe::verified(&info))
        }
        None => media_like(e),
    };
    eps.iter()
        .find(|e| playable(e))
        .or_else(|| eps.first())
        .map(|e| e.line_name.clone())
}

/// 原生直链只能交付真实媒体。既没有识别出视频、地址也不是媒体文件形态的非 HLS 线路
/// 是需要网页解析的播放页或分享页，不作为备选来源；地址像媒体但画质未知的仍排在最后。
fn deliverable(e: &Episode, info: &probe::MediaInfo) -> bool {
    probe::require_available(info).is_ok()
        && (info.kind == "hls" || probe::verified(info) || media_like(e))
}

async fn work_episodes(app: &App, who: &Identity, film: &Film) -> Result<Vec<Episode>> {
    let work = identity::work(app, film).await?;
    let mut members = identity::members(app, &work.group_key).await?;
    let preferred = identity::preferred(app, who, &work).await?;
    members.sort_by_key(|film| film.id != preferred.id);
    let mut numbered = BTreeMap::new();
    let mut ambiguous = Vec::new();
    for member in members {
        if !access::source_allowed(app, &who.user.id, &member.source_id).await?
            || !catalog::source(app, &member.source_id).await?.active()
        {
            continue;
        }
        let eps = selected_line_episodes(app, who, &work.season_id, &member.id).await?;
        let mut counts = HashMap::new();
        for e in &eps {
            if let Some(number) = catalog::episode_number(&e.title) {
                *counts.entry(number).or_insert(0) += 1;
            }
        }
        for e in eps {
            if let Some(number) =
                catalog::episode_number(&e.title).filter(|number| counts[number] == 1)
            {
                numbered.entry(number).or_insert(e);
            } else if member.id == preferred.id {
                ambiguous.push(e);
            }
        }
    }
    let mut episodes: Vec<_> = numbered.into_values().collect();
    episodes.extend(ambiguous);
    Ok(episodes)
}
async fn row(
    app: &App,
    who: &Identity,
    f: &Film,
    episode: Option<&Episode>,
    is_season: bool,
) -> Result<Value> {
    let work = identity::work(app, f).await?;
    let view = catalog::view(app, &who.user.id, &f.id, false).await?;
    let authorized = view["authorized"] == true;
    let eps = if authorized {
        if f.kind == "TV" {
            work_episodes(app, who, f).await?
        } else {
            selected_line_episodes(app, who, &work.season_id, &f.id).await?
        }
    } else {
        Vec::new()
    };
    let first = eps
        .iter()
        .find(|e| view["progress"]["episode_id"] == e.id)
        .or_else(|| eps.first());
    let count = eps.len();
    let ep = if authorized { episode.or(first) } else { None };
    let info = ep
        .and_then(|e| serde_json::from_str::<probe::MediaInfo>(&e.probe).ok())
        .unwrap_or_default();
    let kind = if is_season {
        "Season"
    } else if episode.is_some() && f.kind == "TV" {
        "Episode"
    } else {
        f.kind.as_str()
    };
    let id = if is_season {
        work.season_id.clone()
    } else if kind == "Episode" {
        let episode = episode.unwrap();
        identity::episode_guid(app, &catalog::film(app, &episode.film_id).await?, episode).await?
    } else {
        work.id.clone()
    };
    let parent = match kind {
        "Episode" => work.season_id.clone(),
        "Season" => work.id.clone(),
        _ => library(&f.kind),
    };
    let title = if is_season {
        "第 1 季".to_owned()
    } else if kind == "Episode" {
        episode.unwrap().title.clone()
    } else {
        f.title.clone()
    };
    let poster = if !authorized || f.poster.is_empty() {
        String::new()
    } else {
        format!("/tv-poster/{}", f.id)
    };
    let position =
        if !authorized || view["progress"]["episode_id"].as_str() == ep.map(|e| e.id.as_str()) {
            view["progress"]["position"].as_f64().unwrap_or(0.0)
        } else {
            0.0
        };
    let resolution = native_resolution(&info);
    let favored: Vec<String> = sqlx::query_scalar("SELECT film_id FROM favorites WHERE user_id=?")
        .bind(&who.user.id)
        .fetch_all(&app.db)
        .await?;
    let favorite = identity::members(app, &work.group_key)
        .await?
        .iter()
        .any(|film| favored.contains(&film.id));
    Ok(
        json!({"guid":id,"title":title,"type":kind,"lan":"zh-CN","tv_title":f.title,"parent_title":if kind=="Episode"{"第 1 季"}else{f.title.as_str()},
        "original_title":f.title,"imdb_id":"","trim_id":"","poster":poster,"posters":poster,"backdrops":"","vote_average":"0","genres":[],
        "release_date":f.year,"overview":text(&view,"overview"),"is_favorite":u8::from(favorite),
        "is_watched":0,"watched":0,"watched_ts":position as u64,"season_number":1,"episode_number":episode.or(if is_season{ep}else{None}).map(|e|catalog::episode_number(&e.title).map(i64::from).unwrap_or(e.number)).unwrap_or(0),
        "number_of_seasons":if f.kind=="TV"{1}else{0},"number_of_episodes":count,"local_number_of_episodes":count,"local_number_of_seasons":if f.kind=="TV"{1}else{0},
        "can_play":u8::from(view["available"]==true && kind!="Season"),"play_error":text(&view,"unavailable_reason"),
        "parent_guid":parent,"grand_guid":if f.kind=="TV"{work.id.as_str()}else{""},"ancestor_guid":library(&f.kind),"ancestor_name":if f.kind=="TV"{"剧集"}else{"电影"},
        "ancestor_category":f.kind,"play_item_guid":ep.map(|e|e.id.as_str()).unwrap_or(""),"media_guid":ep.map(|e|e.id.as_str()).unwrap_or(""),
        "duration":info.duration as u64,"logic_type":0,"single_child_guid":"","file_name":ep.map(|e|format!("{}.{}",e.title,if info.kind=="hls"{"m3u8"}else{"mp4"})).unwrap_or_default(),
        "create_time":f.updated_at*1000,"update_time":f.updated_at*1000,"first_air_date":"","last_air_date":"","air_date":"","status":"","douban_id":0,
        "poster_list":if poster.is_empty(){vec![]}else{vec![poster]},"media_stream":{"resolutions":if resolution.is_empty(){vec![]}else{vec![resolution]},"audio_type":null,"color_range_type":[]}}),
    )
}
fn file(e: &Episode, info: &probe::MediaInfo) -> Value {
    json!({"guid":e.id,"path":format!("/tv-rust/{}.{}",e.id,if info.kind=="hls"{"m3u8"}else{"mp4"}),"file_name":format!("{}.{}",e.title,if info.kind=="hls"{"m3u8"}else{"mp4"}),
        "size":info.size,"timestamp":now()*1000,"type":1,"can_play":1,"play_error":"","create_time":now()*1000,"update_time":now()*1000,"file_birth_time":now()*1000,"progress_thumb_hash_dir":""})
}
fn quality_rank(info: &probe::MediaInfo) -> (u64, u64) {
    let quality = probe::quality(info);
    let width = quality["width"].as_f64().unwrap_or(0.0) as u64;
    let height = quality["height"].as_f64().unwrap_or(0.0) as u64;
    if width == 0 || height == 0 {
        (0, 0)
    } else {
        (height, width)
    }
}
pub(crate) fn quality_label(info: &probe::MediaInfo) -> String {
    let (height, width) = quality_rank(info);
    if height == 0 {
        return "画质待识别".to_owned();
    }
    let quality = probe::quality(info);
    let mut parts = vec![format!("{height}p"), format!("{width} × {height}")];
    let codec = text(&quality, "codec");
    if !codec.is_empty() {
        parts.push(match codec.as_str() {
            "h264" => "H.264".to_owned(),
            "hevc" | "h265" => "H.265".to_owned(),
            _ => codec.to_uppercase(),
        });
    }
    let rate = quality["frame_rate"].as_f64().unwrap_or(0.0);
    if rate > 0.0 {
        parts.push(format!(
            "{} fps",
            format!("{rate:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        ));
    }
    parts.join(" · ")
}
fn native_resolution(info: &probe::MediaInfo) -> String {
    let (height, _) = quality_rank(info);
    match height {
        0 => String::new(),
        4320.. => "8k".to_owned(),
        2160.. => "4k".to_owned(),
        1440.. => "2k".to_owned(),
        _ => format!("{height}p"),
    }
}
pub(crate) fn line_label(e: &Episode) -> String {
    if e.line_name
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        e.line_name.clone()
    } else {
        format!("线路 {}", e.line_order + 1)
    }
}
async fn source_file(app: &App, user: &str, e: &Episode, info: &probe::MediaInfo) -> Result<Value> {
    let (fresh, _, source) = catalog::available(app, user, &e.id).await?;
    if fresh.url != e.url {
        return Err(Error::bad("播放地址已更新，请重新加载来源"));
    }
    let mut value = file(e, info);
    let source_path = source.name.replace(['/', '\\'], "_");
    value["path"] = json!(format!(
        "/视频源/{source_path}/{} · {}/{}.{}",
        line_label(e),
        quality_label(info),
        e.title.replace(['/', '\\'], "_"),
        if info.kind == "hls" { "m3u8" } else { "mp4" }
    ));
    value["file_name"] = json!(format!(
        "{} · {} · {} · {}",
        source.name,
        line_label(e),
        e.title,
        quality_label(info)
    ));
    value["source_id"] = json!(source.id);
    value["source_name"] = json!(source.name);
    value["line_name"] = json!(line_label(e));
    value["quality"] = probe::quality(info);
    Ok(value)
}

pub(crate) fn menu_label(file: &Value, duplicated: bool) -> String {
    let name = text(file, "source_name");
    // 飞牛的三列按钮不会自动换行，菜单只保留来源和必要的线路编号。
    let name = name.trim().trim_start_matches('🎬').trim();
    let line = if duplicated {
        format!("/{}", text(file, "line_name").trim_start_matches("线路 "))
    } else {
        String::new()
    };
    format!("{name}{line}")
}

fn playback_mode_label(mode: &str, pending: bool) -> String {
    let label = if mode == "proxy" { "转发" } else { "直链" };
    if pending {
        format!("{label}（待播放）")
    } else {
        label.to_owned()
    }
}

fn show_file_playback_mode(file: &mut Value, mode: &str, pending: bool) {
    let label = playback_mode_label(mode, pending);
    file["file_name"] = json!(format!("{label} · {}", text(file, "file_name")));
    file["path"] = json!(format!("/播放方式/{label}{}", text(file, "path")));
    file["playback_mode"] = json!(mode);
    file["playback_mode_label"] = json!(label);
    file["playback_mode_pending"] = json!(pending);
}

fn show_video_playback_mode(video: &mut Value, mode: &str, pending: bool) {
    video["title"] = json!(format!(
        "{} · {}",
        playback_mode_label(mode, pending),
        text(video, "title")
    ));
}

pub(crate) async fn stream_list(
    app: &App,
    who: &Identity,
    reference: &str,
    refresh: bool,
) -> Result<Value> {
    let e = resolve_episode(app, who, reference).await?;
    // 打开详情页时最多等三秒；没识别完的来源在后台继续，起播时通常已经就绪。
    // 用户主动刷新时等满期限。
    let budget = Duration::from_secs(8);
    let wait = if refresh {
        budget
    } else {
        Duration::from_secs(3)
    };
    let mut result = source_choices_within(app, who, &e.id, refresh, budget, wait).await?;
    let choices = result["files"].as_array().cloned().unwrap_or_default();
    result["qualities"] = json!(choices.iter().enumerate().map(|(index,file)|json!({
        "resolution":menu_label(file,choices.iter().filter(|other|other["source_id"]==file["source_id"]).count()>1),"bitrate":90_000_000 + index as u64 * 10001,"progressive":true,"is_m3u8":false
    })).collect::<Vec<_>>());
    result["files"] = json!(
        choices
            .into_iter()
            .filter(|file| file["guid"] == e.id)
            .collect::<Vec<_>>()
    );
    for key in ["video_streams", "audio_streams", "subtitle_streams"] {
        if let Some(streams) = result[key].as_array_mut() {
            streams.retain(|stream| stream["media_guid"] == e.id);
        }
    }
    let play = playback::latest(app, who, &e.id).await.ok();
    let hls = text(&result["files"][0], "path").ends_with(".m3u8");
    native_playback::warm_direct_hls(app, who, &e, hls);
    let mode = play.as_ref().map(|play| play.mode.as_str()).unwrap_or(
        if hls && !*app.forbid_proxy.borrow() {
            "proxy"
        } else {
            "direct"
        },
    );
    let pending = play.is_none();
    if let Some(files) = result["files"].as_array_mut() {
        for file in files {
            show_file_playback_mode(file, mode, pending);
        }
    }
    if let Some(videos) = result["video_streams"].as_array_mut() {
        for video in videos {
            show_video_playback_mode(video, mode, pending);
        }
    }
    result["playback_mode"] = json!(mode);
    result["playback_mode_label"] = json!(playback_mode_label(mode, pending));
    result["playback_mode_pending"] = json!(pending);
    result["cloud_storage_info"] = Value::Null;
    Ok(result)
}

/// 后台识别结束时把单集移出进行中集合，任务被取消或出错时同样生效。
struct Probing(App, String);
impl Drop for Probing {
    fn drop(&mut self) {
        self.0.probing.lock().unwrap().remove(&self.1);
    }
}

/// `budget` 限制来源发现与识别的总时间；`probe_wait` 是其中等待画质识别的最长时间。
/// 起播时只短暂等待，识别在后台继续并保存，下一次读取菜单时直接可用。
pub(crate) async fn source_choices_within(
    app: &App,
    who: &Identity,
    reference: &str,
    refresh: bool,
    budget: Duration,
    probe_wait: Duration,
) -> Result<Value> {
    let e = resolve_episode(app, who, reference).await?;
    let (_, current, _) = catalog::available(app, &who.user.id, &e.id).await?;
    // 原生列表一次返回。限制整个发现与识别的时间，慢源不能无限拖住播放界面。
    let deadline = Instant::now() + budget;
    let mut choices = BTreeMap::from([(e.id.clone(), e.clone())]);
    let discovered = timeout_at(
        (Instant::now() + Duration::from_secs(2)).min(deadline),
        catalog::playback_sources(app, &who.user.id, &current.id, true, refresh),
    )
    .await;
    let candidates = match discovered {
        Ok(Ok(value)) => value,
        _ => catalog::playback_sources(app, &who.user.id, &current.id, false, false).await?,
    };
    let mut ids = vec![current.id.clone()];
    ids.extend(
        candidates["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| text(item, "id"))
            .filter(|id| id != &current.id),
    );
    {
        let prepare = stream::iter(ids.into_iter().map(|id| {
            let e = &e;
            let current = &current;
            async move {
                let target = catalog::detail(app, &who.user.id, &id, refresh).await?;
                let lines: BTreeMap<_, _> = catalog::episodes(app, &id)
                    .await?
                    .into_iter()
                    .map(|item| (item.line_order, item.line_name))
                    .collect();
                let mut episodes = Vec::new();
                for line in lines.into_values() {
                    let hint = catalog::switch_episode_hint(
                        app,
                        &who.user.id,
                        &target.id,
                        &current.id,
                        &e.id,
                        Some(&line),
                        0.0,
                    )
                    .await?;
                    if hint["matched"] == true {
                        episodes.push(
                            catalog::available(app, &who.user.id, &text(&hint, "episode_id"))
                                .await?
                                .0,
                        );
                    }
                }
                Ok::<_, Error>(episodes)
            }
        }))
        .buffer_unordered(4);
        tokio::pin!(prepare);
        while let Ok(Some(result)) = timeout_at(deadline, prepare.next()).await {
            if let Ok(episodes) = result {
                for episode in episodes {
                    if choices.contains_key(&episode.id) || choices.len() < 100 {
                        choices.insert(episode.id.clone(), episode);
                    }
                }
            }
        }
    }
    // 识别结果过期后先沿用同一地址上一次的结论，重新识别完成再覆盖；
    // 菜单不会因为缓存到期而整体退回“画质待识别”。
    let mut info: HashMap<_, _> = choices
        .iter()
        .map(|(id, episode)| (id.clone(), probe::last_known(episode).unwrap_or_default()))
        .collect();
    // 缓存命中立即显示；剩余来源共享后台探测上限，给实际播放保留容量。
    // 识别任务独立于本次请求：慢源超过等待时间后继续识别并保存结果，
    // 不会每次打开菜单都从头再等一遍。
    {
        let (results, mut finished) = tokio::sync::mpsc::unbounded_channel();
        for episode in choices.values() {
            if !refresh && probe::cached(episode).is_some() {
                continue;
            }
            if !app.probing.lock().unwrap().insert(episode.id.clone()) {
                continue;
            }
            let guard = Probing(app.clone(), episode.id.clone());
            let (app, user, episode, results) = (
                app.clone(),
                who.user.id.clone(),
                episode.clone(),
                results.clone(),
            );
            tokio::spawn(async move {
                let measured = probe::source_quality(&app, &user, &episode, refresh, false).await;
                drop(guard);
                let _ = results.send((episode, measured));
            });
        }
        drop(results);
        let wait = deadline.min(Instant::now() + probe_wait);
        while let Ok(Some((episode, measured))) = timeout_at(wait, finished.recv()).await {
            match measured {
                Ok(measured) => {
                    info.insert(episode.id.clone(), measured);
                }
                // 确认是网页或图片的地址记下原因，后面据此从菜单中排除。
                Err(error) if error.1 == probe::WEB_PAGE || error.1 == probe::IMAGE => {
                    info.insert(
                        episode.id.clone(),
                        probe::MediaInfo {
                            kind: "video".into(),
                            warning: error.1,
                            ..Default::default()
                        },
                    );
                }
                Err(_) => {}
            }
        }
    }
    catalog::available(app, &who.user.id, &e.id).await?;
    let mut files = Vec::new();
    let mut videos = Vec::new();
    let mut audios = Vec::new();
    let mut subtitles = Vec::new();
    let mut choices: Vec<_> = choices.into_values().collect();
    // 禁止转发时已知加密的 HLS 无法直链播放，排到可播放来源之后。
    let forbidden = *app.forbid_proxy.borrow();
    let source_health = catalog::source_health_ranks(app, &choices).await?;
    let health_rank = |episode: &Episode| source_health.get(&episode.film_id).copied().unwrap_or(1);
    choices.sort_by(|a, b| {
        (forbidden && info[&a.id].encrypted)
            .cmp(&(forbidden && info[&b.id].encrypted))
            .then(quality_rank(&info[&b.id]).cmp(&quality_rank(&info[&a.id])))
            .then_with(|| health_rank(a).cmp(&health_rank(b)))
            .then(a.id.cmp(&b.id))
    });
    for episode in choices {
        let measured = &info[&episode.id];
        if episode.id != e.id && !deliverable(&episode, measured) {
            continue;
        }
        match source_file(app, &who.user.id, &episode, measured).await {
            Ok(value) => files.push(value),
            Err(error) if episode.id == e.id => return Err(error),
            Err(_) => continue,
        }
        let source_name = text(files.last().unwrap(), "source_name");
        let (video, audio, subtitle) = tracks(&episode, measured, &source_name);
        videos.extend(video);
        audios.extend(audio);
        subtitles.extend(subtitle);
    }
    catalog::available(app, &who.user.id, &e.id).await?;
    let allowed: HashSet<_> = access::sources(app, &who.user.id)
        .await?
        .into_iter()
        .filter(|source| source.active())
        .map(|source| source.id)
        .collect();
    files.retain(|file| allowed.contains(&text(file, "source_id")));
    let retained: HashSet<_> = files.iter().map(|file| text(file, "guid")).collect();
    videos.retain(|video| retained.contains(&text(video, "media_guid")));
    audios.retain(|audio| retained.contains(&text(audio, "media_guid")));
    subtitles.retain(|subtitle| retained.contains(&text(subtitle, "media_guid")));
    Ok(
        json!({"files":files,"video_streams":videos,"audio_streams":audios,"subtitle_streams":subtitles}),
    )
}
async fn source_poster(app: &App, who: &Identity, path: &str) -> Result<Response> {
    let id = path.split('/').next().ok_or_else(Error::missing)?;
    let view = source_season_info(app, id)
        .await?
        .ok_or_else(Error::missing)?;
    let (e, f, _) = catalog::available(app, &who.user.id, &text(&view, "episode_id")).await?;
    if e.film_id != text(&view, "film_id") || e.line_name != text(&view, "line_name") {
        return Err(Error::missing());
    }
    crate::api::poster(app, &who.user.id, &f.id).await
}
pub(crate) async fn validate_item_media(
    app: &App,
    who: &Identity,
    item: &str,
    e: &Episode,
) -> Result<()> {
    if item.is_empty() || item == e.id || item == e.film_id || item == season(&e.film_id) {
        return Ok(());
    }
    if let Some(matched) = identity::matches_episode(app, item, e).await? {
        return if matched {
            Ok(())
        } else {
            Err(Error::bad("播放来源与原单集不匹配，请重新选集"))
        };
    }
    if let Some(view) = source_season_info(app, item).await?
        && text(&view, "film_id") == e.film_id
        && text(&view, "line_name") == e.line_name
    {
        return Ok(());
    }
    let original = resolve_episode(app, who, item).await?;
    let hint = catalog::switch_episode_hint(
        app,
        &who.user.id,
        &e.film_id,
        &original.film_id,
        &original.id,
        Some(&e.line_name),
        0.0,
    )
    .await?;
    if hint["matched"] != true || text(&hint, "episode_id") != e.id {
        return Err(Error::bad("播放来源与原单集不匹配，请重新选集"));
    }
    Ok(())
}
pub(crate) async fn finish_source_switch(
    app: &App,
    who: &Identity,
    f: &Film,
    play_id: &str,
) -> Result<()> {
    let current = playback::owned(app, who, play_id).await?;
    let ids = catalog::history_film_ids(app, &who.user.id, &f.id).await?;
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM plays WHERE auth_hash=");
    query
        .push_bind(&who.auth_hash)
        .push(" AND user_id=")
        .push_bind(&who.user.id)
        .push(" AND ended=0 AND id<>")
        .push_bind(play_id)
        .push(" AND seq<")
        .push_bind(current.seq)
        .push(" AND film_id IN (");
    let mut films = query.separated(",");
    for id in &ids {
        films.push_bind(id);
    }
    query.push(")");
    let previous = query
        .build_query_as::<playback::Play>()
        .fetch_all(&app.db)
        .await?;
    for p in previous {
        playback::end(app, &p).await?;
    }
    Ok(())
}
fn tracks(
    e: &Episode,
    info: &probe::MediaInfo,
    source_name: &str,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut streams = info.streams.clone();
    if !streams.iter().any(probe::is_video_stream) {
        streams.push(probe::video(info));
    }
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subtitle = Vec::new();
    for mut v in streams {
        let index = v["index"].as_i64().unwrap_or(0);
        let kind = text(&v, "codec_type");
        if kind == "video" && !probe::is_video_stream(&v) {
            continue;
        }
        v["guid"] = json!(guid(&["track", &e.id, &index.to_string()]));
        v["media_guid"] = json!(e.id);
        v["duration"] = json!(info.duration as u64);
        v["create_time"] = json!(now() * 1000);
        v["update_time"] = json!(now() * 1000);
        v["title"] = json!(text(&v["tags"], "title"));
        v["bps"] = json!(text(&v, "bit_rate").parse::<u64>().unwrap_or(0));
        match kind.as_str() {
            "video" => {
                let measured_stream = probe::MediaInfo {
                    streams: vec![v.clone()],
                    ..Default::default()
                };
                v["title"] = json!(format!(
                    "{} · {} · {}",
                    source_name,
                    line_label(e),
                    quality_label(&measured_stream)
                ));
                v["resolution_type"] = json!(native_resolution(&measured_stream));
                v["color_range_type"] = json!(match text(&v, "color_transfer").as_str() {
                    "smpte2084" => "HDR10",
                    "arib-std-b67" => "HLG",
                    "bt709" | "bt470m" | "bt470bg" | "smpte170m" | "smpte240m" | "iec61966-2-1" =>
                        "SDR",
                    _ => "",
                });
                v["wrapper"] = json!(if info.kind == "hls" {
                    "HLS"
                } else {
                    "mov,mp4,m4a,3gp,3g2,mj2"
                });
                v["progressive"] = json!(1);
                v["level"] = json!(text(&v, "level"));
                v["dv_profile"] = json!(0);
                v["rotation"] = json!(0);
                v["ext1"] = json!(0);
                v["is_bluray"] = json!(false);
                video.push(v);
            }
            "audio" => {
                v["audio_type"] = json!("Stereo");
                v["language"] = json!(text(&v["tags"], "language"));
                v["is_default"] = json!(v["disposition"]["default"].as_i64().unwrap_or(0));
                v["is_fake"] = json!(false);
                audio.push(v);
            }
            "subtitle" => {
                v["language"] = json!(text(&v["tags"], "language"));
                v["is_default"] = json!(0);
                v["forced"] = json!(0);
                v["is_external"] = json!(0);
                v["is_bitmap"] = json!(0);
                subtitle.push(v);
            }
            _ => {}
        }
    }
    (video, audio, subtitle)
}
fn library_row(kind: &str) -> Value {
    json!({"guid":library(kind),"title":if kind=="TV"{"我的剧集"}else{"我的电影"},"category":kind,"view_type":0,"posters":[]})
}
async fn list(app: &App, who: &Identity, v: &Value, favorites: bool) -> Result<Value> {
    let query = text(v, "q");
    let ancestor = text(v, "ancestor_guid");
    let page = v["page"].as_i64().unwrap_or(1).clamp(1, 500);
    let size = v["page_size"].as_i64().unwrap_or(50).clamp(1, 128);
    let mut rows = Vec::new();
    let remote_search = !query.trim().is_empty() && !favorites;
    if remote_search {
        let result = catalog::search(app, &who.user.id, &query, page, None).await?;
        let mut seen = HashSet::new();
        for v in result["items"].as_array().into_iter().flatten() {
            let f = catalog::film(app, &text(v, "id")).await?;
            if seen.insert(identity::key(&f)) {
                rows.push(row(app, who, &f, None, false).await?);
            }
        }
    } else if !ancestor.is_empty() && ancestor != library("Movie") && ancestor != library("TV") {
        let f = authorized_film(app, who, &ancestor).await?;
        let f = catalog::detail(app, &who.user.id, &f.id, false).await?;
        if ancestor == f.id
            || identity::find(app, &ancestor)
                .await?
                .is_some_and(|work| work.id == ancestor)
        {
            rows.push(row(app, who, &f, None, true).await?);
        } else {
            let eps = if identity::find(app, &ancestor).await?.is_some() {
                work_episodes(app, who, &f).await?
            } else {
                selected_line_episodes(app, who, &ancestor, &f.id).await?
            };
            for e in &eps {
                let mut item = row(app, who, &f, Some(e), false).await?;
                item["parent_guid"] = json!(ancestor);
                rows.push(item);
            }
        }
    } else {
        let ids: Vec<String> = if favorites {
            sqlx::query_scalar("SELECT film_id FROM favorites WHERE user_id=?")
                .bind(&who.user.id)
                .fetch_all(&app.db)
                .await?
        } else {
            sqlx::query_scalar("SELECT film_id FROM favorites WHERE user_id=? UNION SELECT film_id FROM progress WHERE user_id=?").bind(&who.user.id).bind(&who.user.id).fetch_all(&app.db).await?
        };
        let mut seen = HashSet::new();
        for id in ids {
            let f = catalog::film(app, &id).await?;
            if (ancestor.is_empty() || ancestor == library(&f.kind))
                && seen.insert(identity::key(&f))
            {
                let work = identity::work(app, &f).await?;
                let selected = identity::preferred(app, who, &work).await.unwrap_or(f);
                rows.push(row(app, who, &selected, None, false).await?);
            }
        }
    }
    if let Some(types) = v["tags"]["type"].as_array().filter(|v| !v.is_empty()) {
        rows.retain(|r| types.contains(&r["type"]));
    }
    if favorites && !query.is_empty() {
        rows.retain(|r| {
            text(r, "title")
                .to_lowercase()
                .contains(&query.to_lowercase())
        });
    }
    if !remote_search {
        if rows.iter().all(|row| row["type"] == "Episode") {
            rows.sort_by_key(|row| row["episode_number"].as_i64().unwrap_or(0));
        } else {
            rows.sort_by_key(|row| text(row, "title"));
        }
    }
    let total = rows.len();
    // 远端搜索已经按源分页；只对本地目录再次分页。
    let start = if query.trim().is_empty() || favorites {
        ((page - 1) * size) as usize
    } else {
        0
    };
    Ok(
        json!({"list":rows.into_iter().skip(start).take(size as usize).collect::<Vec<_>>(),"total":total,"mdb_name":"","mdb_category":"","top_dir":"","dir":"","jump_list":[]}),
    )
}
async fn inner(app: &App, request: Request) -> Result<Response> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    if path == "/trimcon" {
        return Ok(ok(json!({})));
    }
    if path == "/v/api/v1/sys/version" {
        return Ok(ok(
            json!({"version":"0.9.7","mediasrvVersion":"0.8.35","serverName":"TV Rust"}),
        ));
    }
    if path == "/v/api/v1/sys/config" {
        return Ok(ok(json!({"initialized":true})));
    }
    if path == "/v/api/v2/user/loginByPassword" && method == Method::POST {
        let bytes = to_bytes(request.into_body(), 16 * 1024)
            .await
            .map_err(|_| Error::bad("请求过大"))?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("登录请求无效"))?;
        let (token, _) =
            auth::login(app, &text(&v, "username"), &text(&v, "password"), true).await?;
        return Ok(ok(json!({"token":token})));
    }
    let who = auth::identity(app, &headers).await?;
    if method == Method::GET || method == Method::HEAD {
        match path.as_str() {
            "/v/api/v1/search/list" => {
                let q = query.get("q").map(String::as_str).unwrap_or("");
                if q.trim().is_empty() {
                    return Ok(ok(json!([])));
                }
                let result = catalog::search(app, &who.user.id, q, 1, None).await?;
                let mut rows = Vec::new();
                let mut seen = HashSet::new();
                for v in result["items"].as_array().into_iter().flatten() {
                    let f = catalog::film(app, &text(v, "id")).await?;
                    if seen.insert(identity::key(&f)) {
                        rows.push(row(app, &who, &f, None, false).await?);
                    }
                }
                return Ok(ok(json!(rows)));
            }
            "/v/api/v1/user/info" => {
                return Ok(ok(
                    json!({"guid":who.user.id,"username":who.user.username,"is_admin":u8::from(who.user.admin)}),
                ));
            }
            "/v/api/v1/mediadb/list" => {
                return Ok(ok(json!([library_row("Movie"), library_row("TV")])));
            }
            "/v/api/v1/mediadb/sum" => {
                let v = list(app, &who, &json!({"page_size":128}), false).await?;
                let rows = v["list"].as_array().unwrap();
                let movies = rows.iter().filter(|v| v["type"] == "Movie").count();
                let tv = rows.iter().filter(|v| v["type"] == "TV").count();
                let mut v = json!({"total":movies+tv,"movie":movies,"tv":tv,"live":0,"other":0,"Movie":movies,"TV":tv,"Video":0,"Directory":0});
                v[library("Movie")] = json!(movies);
                v[library("TV")] = json!(tv);
                return Ok(ok(v));
            }
            "/v/api/v1/play/list" => {
                let items = catalog::continue_watching_with_limit(app, &who.user.id, 100).await?;
                let mut rows = Vec::new();
                for item in items {
                    let e = catalog::episode(app, &text(&item["progress"], "episode_id")).await?;
                    let f = catalog::film(app, &e.film_id).await?;
                    let mut v = row(app, &who, &f, Some(&e), false).await?;
                    v["ts"] = v["watched_ts"].clone();
                    rows.push(v);
                }
                return Ok(ok(json!(rows)));
            }
            "/v/api/v1/tag/list" => {
                return Ok(ok(
                    json!({"genres":[],"resolutions":[],"color_range":[],"audio_type":[],"locate":[],"decades":[],"recognition_status":[]}),
                ));
            }
            "/v/api/v1/task/running" => return Ok(ok(json!([]))),
            p if p.starts_with("/v/api/v1/tag/") => return Ok(ok(json!([]))),
            _ => {}
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/mdb/") {
            for kind in ["Movie", "TV"] {
                if id == library(kind) {
                    return Ok(ok(library_row(kind)));
                }
            }
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/item/") {
            for kind in ["Movie", "TV"] {
                if id == library(kind) {
                    return Ok(ok(
                        json!({"guid":id,"title":if kind=="TV"{"我的剧集"}else{"我的电影"},"type":"Directory","ancestor_guid":id,"ancestor_name":"个人媒体库","ancestor_category":kind,"parent_guid":"","grand_guid":"","can_play":0,"play_error":"","play_item_guid":"","media_guid":"","single_child_guid":"","file_name":"","duration":0,"logic_type":0,"is_favorite":0,"is_watched":0,"watched":0,"watched_ts":0,"poster":"","posters":"","vote_average":"0","poster_list":[],"tv_title":"","parent_title":"","genres":[],"overview":"收藏与最近观看；可以直接搜索已启用视频源"}),
                    ));
                }
            }
            let f = authorized_film(app, &who, id).await?;
            let f = catalog::detail(app, &who.user.id, &f.id, false)
                .await
                .unwrap_or(f);
            access::require_source(app, &who.user.id, &f.source_id).await?;
            if let Some(view) = source_season_info(app, id).await? {
                let e = resolve_episode(app, &who, id).await?;
                if e.line_name != text(&view, "line_name") {
                    return Err(Error::bad("该播放线路已更新，请重新选择来源"));
                }
                return Ok(ok(row(app, &who, &f, None, true).await?));
            }
            let e = if identity::episode_work(app, id).await?.is_some() {
                Some(resolve_episode(app, &who, id).await?)
            } else {
                catalog::episode(app, id).await.ok()
            };
            let is_season = id == season(&f.id)
                || identity::find(app, id)
                    .await?
                    .is_some_and(|work| work.season_id == id);
            return Ok(ok(row(app, &who, &f, e.as_ref(), is_season).await?));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/season/list/") {
            let f = catalog::detail(
                app,
                &who.user.id,
                &authorized_film(app, &who, id).await?.id,
                false,
            )
            .await?;
            return Ok(ok(json!([row(app, &who, &f, None, true).await?])));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/episode/list/") {
            let f = catalog::detail(
                app,
                &who.user.id,
                &authorized_film(app, &who, id).await?.id,
                false,
            )
            .await?;
            let eps = if identity::find(app, id).await?.is_some() {
                work_episodes(app, &who, &f).await?
            } else {
                selected_line_episodes(app, &who, id, &f.id).await?
            };
            let mut rows = Vec::new();
            for e in &eps {
                let mut item = row(app, &who, &f, Some(e), false).await?;
                item["parent_guid"] = json!(id);
                rows.push(item);
            }
            return Ok(ok(json!(rows)));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/stream/list/") {
            return Ok(ok(stream_list(
                app,
                &who,
                id,
                query.get("refresh").is_some_and(|v| v == "1"),
            )
            .await?));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/media/range/") {
            let e = resolve_episode(app, &who, id).await?;
            if let Some(response) =
                native_playback::range(app, &who, &e, &query, method.clone(), headers.clone())
                    .await?
            {
                return Ok(response);
            }
            catalog::available(app, &who.user.id, &e.id).await?;
            let mut p = playback::latest(app, &who, &e.id).await?;
            if p.mode == "direct" {
                // 原生文件入口也是客户端“NAS 代理播放”的重试通道。
                // 只为该会话建立安全转发，仍遵守全局禁止转发设置。
                playback::fallback(app, &who, &p.id, "manual").await?;
                p = playback::get(app, &p.id).await?;
            }
            native_playback::proxy_menu(
                app,
                &who,
                &e,
                &native_playback::origin(app, &headers)?,
                &p.id,
            )
            .await?;
            let id = playback::asset(app, &p, &e.url).await?;
            return playback::proxy_play(app, &p, None, &id, method, headers).await;
        }
        if let Some(id) = path
            .strip_prefix("/v/api/v1/sys/img/tv-poster/")
            .or_else(|| path.strip_prefix("/tv-poster/"))
        {
            return crate::api::poster(app, &who.user.id, id).await;
        }
        if let Some(id) = path
            .strip_prefix("/v/api/v1/sys/img/fntv-source-poster/")
            .or_else(|| path.strip_prefix("/fntv-source-poster/"))
        {
            return source_poster(app, &who, id).await;
        }
        if path == "/v/api/v1/sys/img"
            && let Some(image) = query.get("path")
        {
            if let Some(id) = image.strip_prefix("/fntv-source-poster/") {
                return source_poster(app, &who, id).await;
            }
            if let Some(id) = image.strip_prefix("/tv-poster/") {
                return crate::api::poster(app, &who.user.id, id).await;
            }
        }
        return Err(Error::missing());
    }
    let bytes = to_bytes(request.into_body(), 32 * 1024)
        .await
        .map_err(|_| Error::bad("请求过大"))?;
    let v: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).map_err(|_| Error::bad("协议请求无效"))?
    };
    match path.as_str() {
        "/v/api/v1/user/logout" => {
            sqlx::query("DELETE FROM auth_sessions WHERE token_hash=?")
                .bind(&who.auth_hash)
                .execute(&app.db)
                .await?;
            return Ok(ok(json!(true)));
        }
        "/v/api/v1/item/list" => return Ok(ok(list(app, &who, &v, false).await?)),
        "/v/api/v1/favorite/list" => return Ok(ok(list(app, &who, &v, true).await?)),
        "/v/api/v1/user/getData" | "/v/api/v1/user/setData" => {
            let key = format!("{}:{}", text(&v, "mdb_guid"), text(&v, "key"));
            if key.len() > 512 {
                return Err(Error::bad("偏好键过长"));
            }
            if path.ends_with("getData") {
                let saved: Option<String> =
                    sqlx::query_scalar("SELECT value FROM preferences WHERE user_id=? AND key=?")
                        .bind(&who.user.id)
                        .bind(&key)
                        .fetch_optional(&app.db)
                        .await?;
                return Ok(saved
                    .map(|value| ok(json!({"key":v["key"],"mdb_guid":v["mdb_guid"],"value":value})))
                    .unwrap_or_else(|| {
                        Json(json!({"code":-6,"msg":"","data":null})).into_response()
                    }));
            }
            sqlx::query("INSERT INTO preferences VALUES(?,?,?) ON CONFLICT(user_id,key) DO UPDATE SET value=excluded.value").bind(&who.user.id).bind(key).bind(text(&v,"value")).execute(&app.db).await?;
            return Ok(ok(json!(true)));
        }
        "/v/api/v1/play/quit" => {
            let link = text(&v, "play_link");
            if native_playback::quit(app, &who, &link).await? {
                return Ok(ok(json!(true)));
            }
            let p:Option<playback::Play>=sqlx::query_as("SELECT * FROM plays WHERE auth_hash=? AND native_link_hash=? ORDER BY seq DESC LIMIT 1").bind(&who.auth_hash).bind(hash(link)).fetch_optional(&app.db).await?;
            if let Some(p) = p {
                playback::end(app, &p).await?;
            }
            return Ok(ok(json!(true)));
        }
        "/v/api/v1/stream/list" => {
            let reference = ["guid", "item_guid", "media_guid"]
                .into_iter()
                .map(|key| text(&v, key))
                .find(|id| !id.is_empty())
                .ok_or_else(|| Error::bad("请选择要读取的单集"))?;
            return Ok(ok(
                stream_list(app, &who, &reference, v["refresh"] == true).await?
            ));
        }
        "/v/api/v1/media/p" => {
            return Ok(ok(native_playback::rpc(app, &who, &v)
                .await?
                .unwrap_or_else(|| json!([]))));
        }
        _ => {}
    }
    let media = text(&v, "media_guid");
    let item = text(&v, "item_guid");
    let reference = if media.is_empty() {
        item.as_str()
    } else {
        media.as_str()
    };
    if path == "/v/api/v1/play/record" && method == Method::DELETE {
        if reference.is_empty() {
            return Err(Error::bad("请选择要移除的观看记录"));
        }
        let f = resolve_film(app, reference).await?;
        playback::remove_history(app, &who.user.id, &[f.id]).await?;
        return Ok(ok(json!(true)));
    }
    if path == "/v/api/v1/item/favorite" {
        if !matches!(method, Method::PUT | Method::DELETE) {
            return Err(Error::bad("收藏需要 PUT 或 DELETE"));
        }
        let f = if method == Method::PUT {
            authorized_film(app, &who, reference).await?
        } else {
            resolve_film(app, reference).await?
        };
        if method == Method::PUT {
            catalog::favorite(app, &who.user.id, &f.id, true).await?;
        } else {
            let work = identity::work(app, &f).await?;
            let members = identity::members(app, &work.group_key).await?;
            let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
            for film in members {
                sqlx::query("DELETE FROM favorites WHERE user_id=? AND film_id=?")
                    .bind(&who.user.id)
                    .bind(&film.id)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        return Ok(ok(json!(true)));
    }
    if path == "/v/api/v1/item/watched" {
        authorized_film(app, &who, reference).await?;
        return Ok(ok(json!(true)));
    }
    if path == "/v/api/v1/play/record" && native_playback::record(app, &who, &v).await? {
        return Ok(ok(json!(true)));
    }
    let mut e = resolve_episode(app, &who, reference).await?;
    let selection = if matches!(
        path.as_str(),
        "/v/api/v1/stream" | "/v/api/v1/stream/directLink" | "/v/api/v1/play/play"
    ) {
        native_playback::selected(app, &who, &e, &v).await?
    } else {
        None
    };
    if let Some((target, _)) = &selection {
        e = target.clone();
    }
    validate_item_media(app, &who, &item, &e).await?;
    catalog::available(app, &who.user.id, &e.id).await?;
    let prepares = *app.forbid_proxy.borrow()
        && selection.is_none()
        && matches!(
            path.as_str(),
            "/v/api/v1/play/info" | "/v/api/v1/stream" | "/v/api/v1/stream/directLink"
        );
    let mut info = match probe::get(app, &e).await {
        Ok(info) => info,
        // 当前来源确认是网页或图片时先保留原因，交给起播准备寻找同集的可用来源；
        // 找不到替代时仍以这个原因拒绝起播。
        Err(error) if prepares && (error.1 == probe::WEB_PAGE || error.1 == probe::IMAGE) => {
            probe::MediaInfo {
                kind: "video".into(),
                warning: error.1,
                ..Default::default()
            }
        }
        Err(error) => return Err(error),
    };
    let mut preparation_notice = None;
    let mut preparation_from = None;
    let mut client_only = false;
    if prepares {
        let original = e.clone();
        (e, info, preparation_notice, client_only) =
            native_playback::prepare_start(app, &who, &e, &info).await?;
        // 缓存提示不代表再次换源，只承接本次真正换源前的单集位置。
        if path == "/v/api/v1/play/info" && e.id != original.id {
            preparation_from = Some(original);
        }
        validate_item_media(app, &who, &item, &e).await?;
    }
    let f = catalog::film(app, &e.film_id).await?;
    if matches!(
        path.as_str(),
        "/v/api/v1/play/info"
            | "/v/api/v1/stream"
            | "/v/api/v1/stream/directLink"
            | "/v/api/v1/play/play"
    ) {
        if !client_only && probe::client_only(&info).is_some() {
            client_only = native_playback::client_only_delivery(app, &who, &e).await?;
        }
        probe::require_native_available(&info, &e.url, client_only && *app.forbid_proxy.borrow())?;
    }
    catalog::available(app, &who.user.id, &e.id).await?;
    let source = catalog::source(app, &f.source_id).await?;
    let (video, audio, subtitle) = tracks(&e, &info, &source.name);
    match path.as_str() {
        "/v/api/v1/play/info" => {
            let prior: Option<(String, f64, i64)> = sqlx::query_as(
                "SELECT episode_id,position,updated_at FROM progress WHERE user_id=? AND film_id=?",
            )
            .bind(&who.user.id)
            .bind(&f.id)
            .fetch_optional(&app.db)
            .await?;
            let mut ts = 0.0;
            let mut target_updated_at = None;
            if let Some((previous, position, updated_at)) = prior {
                let same_episode = previous == e.id || {
                    let hint = catalog::switch_episode_hint(
                        app,
                        &who.user.id,
                        &f.id,
                        &f.id,
                        &previous,
                        Some(&e.line_name),
                        position,
                    )
                    .await?;
                    hint["matched"] == true && text(&hint, "episode_id") == e.id
                };
                if same_episode {
                    ts = position;
                    target_updated_at = Some(updated_at);
                }
            }
            let mut resumed = false;
            if let Some(original) = preparation_from {
                let prior: Option<(f64, i64)> = sqlx::query_as(
                    "SELECT position,updated_at FROM progress WHERE user_id=? AND film_id=? AND episode_id=?",
                )
                .bind(&who.user.id)
                .bind(&original.film_id)
                .bind(&original.id)
                .fetch_optional(&app.db)
                .await?;
                // 先按记录位置排除片尾，避免回退后把已看完的内容误作续播。
                if let Some((position, updated_at)) = prior
                    && target_updated_at.is_none_or(|target| updated_at > target)
                    && (info.duration <= 0.0 || position < (info.duration - 30.0).max(0.0))
                {
                    ts = (position - 5.0).max(0.0);
                    resumed = true;
                }
            }
            if info.duration > 0.0 {
                ts = ts.min(info.duration);
            }
            if resumed && let Some(notice) = preparation_notice.as_mut() {
                let seconds = ts as u64;
                notice.push_str(&format!(
                    "，并从原来源的 {} 分 {} 秒继续",
                    seconds / 60,
                    seconds % 60
                ));
                native_playback::update_preparation_notice(app, &who, &e.id, notice).await?;
            }
            let mut item_row = row(app, &who, &f, Some(&e), false).await?;
            let work = identity::work(app, &f).await?;
            let parent = if f.kind == "TV" {
                work.season_id.clone()
            } else {
                library("Movie")
            };
            item_row["parent_guid"] = json!(parent);
            Ok(ok(
                json!({"guid":item_row["guid"],"grand_guid":if f.kind=="TV"{work.id}else{String::new()},"parent_guid":parent,"type":if f.kind=="TV"{"Episode"}else{"Movie"},
                "media_guid":e.id,"video_guid":video.first().map(|v|text(v,"guid")).unwrap_or_default(),"audio_guid":audio.first().map(|v|text(v,"guid")).unwrap_or_default(),"subtitle_guid":"","direct_link_audio_index":0,"ts":ts as u64,
                "play_config":{"skip_opening":null,"skip_ending":null},"item":item_row}),
            ))
        }
        "/v/api/v1/stream" | "/v/api/v1/stream/directLink" => {
            tracing::debug!(
                fields = ?v.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                has_play_link = !text(&v, "play_link").is_empty(),
                "飞牛播放协商字段"
            );
            // 手动为当前飞牛会话选择的转发只应用于该次播放；普通新播放仍优先直链。
            let previous = playback::latest(app, &who, &e.id).await.ok().filter(|p| {
                selection.is_some()
                    || p.mode == "proxy" && p.state == "fallback" && !*app.forbid_proxy.borrow()
            });
            let (delivered, play_id) = if let Some(ref p) = previous {
                if let Some((_, link)) = &selection {
                    (link.clone(), p.id.clone())
                } else {
                    playback::proxy_allowed(app, p).await?;
                    let access = random();
                    sqlx::query("UPDATE plays SET access_hash=? WHERE id=?")
                        .bind(hash(&access))
                        .bind(&p.id)
                        .execute(&app.db)
                        .await?;
                    let asset = playback::asset(app, p, &e.url).await?;
                    let base = native_playback::origin(app, &headers)?;
                    playback::event(app, p, "proxy_issued", "已为飞牛当前播放交付转发地址").await?;
                    (
                        format!("{}/media/{access}/{asset}", base.trim_end_matches('/')),
                        p.id.clone(),
                    )
                }
            } else {
                let started = playback::start_native(app, &who, &e.id, client_only).await?;
                let play_id = text(&started, "id");
                // 飞牛 STRM 以交付的 URL 作为 play_link。片段给每次播放独立身份，
                // 不改变上游 HTTP 地址或签名，也不会将本站凭据交给视频源。
                let mut url = net::valid_url(&e.url)?;
                let original = url.fragment().unwrap_or("");
                let fragment = format!(
                    "{original}{}tv-rust-play={play_id}",
                    if original.is_empty() { "" } else { "&" }
                );
                url.set_fragment(Some(&fragment));
                (url.to_string(), play_id)
            };
            sqlx::query("UPDATE plays SET native_link_hash=?,state=CASE WHEN mode='proxy' AND state='fallback' THEN 'issued' ELSE state END WHERE id=?")
                .bind(hash(&delivered))
                .bind(&play_id)
                .execute(&app.db)
                .await?;
            finish_source_switch(app, &who, &f, &play_id).await?;
            let mut response = json!({"file_stream":source_file(app,&who.user.id,&e,&info).await?,"video_stream":video.first(),"audio_streams":audio,"subtitle_streams":subtitle,
                "qualities":[],"direct_link_qualities":[{"bitrate":probe::quality(&info)["bitrate"].as_f64().unwrap_or(0.0) as u64,"resolution":quality_label(&info),"progressive":false,"is_m3u8":info.kind=="hls","url":delivered}],
                "quality":probe::quality(&info),"source_name":source.name,
                "header":{},"direct_link_audio_streams":[],"cloud_storage_info":{"cloud_nick_name":source.name,"is_vip":false,"cloud_storage_type":9001}});
            let menu = if let Some((_, link)) = &selection {
                native_playback::payload_for_link(app, &who, link).await?
            } else {
                native_playback::stream_menu(
                    app,
                    &who,
                    &e,
                    &native_playback::origin(app, &headers)?,
                    &play_id,
                    client_only,
                )
                .await?
            };
            if let Some(menu) = menu {
                response["qualities"] = menu["qualities"].clone();
                response["direct_link_qualities"] = menu["direct_link_qualities"].clone();
                response["direct_link_quality_index"] = menu["selected_quality_index"].clone();
                // 飞牛用视频流的清晰度和选择码定位当前菜单项；实测码率仍由
                // quality 和详情的 stream/list 提供，不将选择码当成媒体码率。
                let current = &menu["selected_quality"];
                response["video_stream"]["resolution_type"] = current["resolution"].clone();
                response["video_stream"]["bps"] = current["bitrate"].clone();
                if info.kind == "hls" && !*app.forbid_proxy.borrow() {
                    response["cloud_storage_info"] = Value::Null;
                } else {
                    // 飞牛 1.13.0 在 STRM（9001）界面不显示画质按钮，来源菜单无从打开。
                    // 直链菜单改报通用的直连类型：播放器显示当前来源和“视频质量”菜单，
                    // 起播失败时也提供“播放其他画质”；媒体仍按空请求头直接读取。
                    response["cloud_storage_info"]["cloud_storage_type"] =
                        json!(DIRECT_MENU_STORAGE_TYPE);
                }
            }
            let mut play = playback::get(app, &play_id).await?;
            if let Some(notice) = preparation_notice {
                playback::event(
                    app,
                    &play,
                    if client_only {
                        "client_direct_issued"
                    } else {
                        "direct_source_selected"
                    },
                    &notice,
                )
                .await?;
                response["playback_notice"] = json!(notice);
                native_playback::clear_preparation(app, &who, &e.id).await?;
            }
            if info.kind == "hls" && !*app.forbid_proxy.borrow() && play.mode == "direct" {
                // 原生 HLS 通道需要携带本站认证读取同源资源；在协商时记录所选方式，
                // 客户端的文件信息与后续实际媒体交付保持一致。
                playback::fallback(app, &who, &play_id, "headers").await?;
                play = playback::get(app, &play_id).await?;
                sqlx::query("UPDATE plays SET state='issued' WHERE id=? AND mode='proxy'")
                    .bind(&play_id)
                    .execute(&app.db)
                    .await?;
                playback::event(app, &play, "proxy_issued", "已为飞牛原生 HLS 准备转发").await?;
            }
            show_file_playback_mode(&mut response["file_stream"], &play.mode, false);
            show_video_playback_mode(&mut response["video_stream"], &play.mode, false);
            response["playback_mode"] = json!(play.mode);
            response["playback_mode_label"] = json!(playback_mode_label(&play.mode, false));
            response["playback_mode_pending"] = json!(false);
            Ok(ok(response))
        }
        "/v/api/v1/play/play" => {
            if let Some(link) = selection
                .map(|(_, link)| link)
                .or(native_playback::current_link(app, &who, &e.id).await?)
            {
                let link =
                    native_playback::client_link(&link, &info.kind, *app.forbid_proxy.borrow())?;
                return Ok(ok(
                    json!({"play_link":link,"media_guid":e.id,"video_guid":video.first().map(|v|text(v,"guid")).unwrap_or_default(),
                    "video_index":0,"audio_guid":audio.first().map(|v|text(v,"guid")).unwrap_or_default(),"audio_index":0,
                    "subtitle_guid":"","subtitle_index":-1,"subtitle_link":"","wrapper":if info.kind=="hls"{"HLS"}else{"MP4"},"hls_time":6,"is_subtitle_external":0,
                    "non_fatal_errno":null,"video_encoder":"","not_supported_resolution":"","supported_highest_resolution":""}),
                ));
            }
            let p = match playback::latest(app, &who, &e.id).await {
                Ok(p) => p,
                Err(_) => {
                    let start = playback::start(app, &who, &e.id).await?;
                    playback::get(app, &text(&start, "id")).await?
                }
            };
            let link = guid(&[&random()]);
            sqlx::query("UPDATE plays SET native_link_hash=? WHERE id=?")
                .bind(hash(&link))
                .bind(&p.id)
                .execute(&app.db)
                .await?;
            Ok(ok(json!({"play_link":link})))
        }
        "/v/api/v1/play/record" => {
            let position = v["ts"].as_f64().ok_or_else(|| Error::bad("观看位置无效"))?;
            if let Ok(p) = playback::latest(app, &who, &e.id).await {
                let link = text(&v, "play_link");
                let bound: Option<i64> = sqlx::query_scalar(
                    "SELECT seq FROM plays WHERE auth_hash=? AND native_link_hash=? ORDER BY seq DESC LIMIT 1",
                ).bind(&who.auth_hash).bind(hash(&link)).fetch_optional(&app.db).await?;
                let saved = playback::record(app, &p, position, info.duration, Some(&link)).await?;
                tracing::debug!(
                    seq = p.seq, previous_binding = bound, saved,
                    link_length = link.len(), link_is_url = link.starts_with("http"),
                    position,
                    fields = ?v.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                    "飞牛进度关联结果"
                );
            }
            Ok(ok(json!(true)))
        }
        _ => Err(Error::missing()),
    }
}
