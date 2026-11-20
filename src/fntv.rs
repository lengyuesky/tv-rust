//! 飞牛播放器协议适配。兼容版本字段仅用于协商，不表示本项目是 fnOS。
use crate::{
    access, auth,
    catalog::{self, Episode, Film},
    core::*,
    net, playback, probe,
};
use axum::{
    Json,
    body::{Body, to_bytes},
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
async fn source_season(app: &App, film: &str, line: &str, episode: &str) -> Result<String> {
    let id = guid(&["fntv-source-season", film, line, episode]);
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fntv_source_versions WHERE id=?)")
            .bind(&id)
            .fetch_one(&app.db)
            .await?;
    if !known {
        sqlx::query("INSERT OR IGNORE INTO fntv_source_versions VALUES(?,?,?,?)")
            .bind(&id)
            .bind(film)
            .bind(line)
            .bind(episode)
            .execute(&app.db)
            .await?;
    }
    Ok(id)
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
    let f = resolve_film(app, id).await?;
    access::require_source(app, &who.user.id, &f.source_id).await?;
    Ok(f)
}
async fn resolve_episode(app: &App, who: &Identity, id: &str) -> Result<Episode> {
    if let Ok(e) = catalog::episode(app, id).await {
        return Ok(catalog::available(app, &who.user.id, &e.id).await?.0);
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
        .or_else(|| eps.first().map(|e| e.line_name.clone()));
    Ok(eps
        .into_iter()
        .filter(|e| Some(&e.line_name) == line.as_ref())
        .collect())
}
async fn row(
    app: &App,
    who: &Identity,
    f: &Film,
    episode: Option<&Episode>,
    is_season: bool,
) -> Result<Value> {
    let view = catalog::view(app, &who.user.id, &f.id, false).await?;
    let authorized = view["authorized"] == true;
    let eps = if authorized {
        catalog::episodes(app, &f.id).await?
    } else {
        Vec::new()
    };
    let first = eps.first();
    let count = eps
        .iter()
        .filter(|e| e.line_order == first.map(|e| e.line_order).unwrap_or(0))
        .count();
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
        season(&f.id)
    } else if kind == "Episode" {
        episode.unwrap().id.clone()
    } else {
        f.id.clone()
    };
    let parent = match kind {
        "Episode" if authorized => {
            source_season(
                app,
                &f.id,
                &episode.unwrap().line_name,
                &episode.unwrap().id,
            )
            .await?
        }
        "Episode" => season(&f.id),
        "Season" => f.id.clone(),
        _ => library(&f.kind),
    };
    let title = if is_season {
        "第 1 季".to_owned()
    } else if kind == "Episode" {
        episode.unwrap().title.clone()
    } else {
        format!("{} · {}", f.title, text(&view, "source_name"))
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
    Ok(
        json!({"guid":id,"title":title,"type":kind,"lan":"zh-CN","tv_title":f.title,"parent_title":text(&view,"source_name"),
        "original_title":f.title,"imdb_id":"","trim_id":"","poster":poster,"posters":poster,"backdrops":"","vote_average":"0","genres":[],
        "release_date":f.year,"overview":format!("来源：{}\n{}",text(&view,"source_name"),text(&view,"overview")),"is_favorite":u8::from(view["favorite"]==true),
        "is_watched":0,"watched":0,"watched_ts":position as u64,"season_number":1,"episode_number":episode.map(|e|catalog::episode_number(&e.title).map(i64::from).unwrap_or(e.number)).unwrap_or(0),
        "number_of_seasons":if f.kind=="TV"{1}else{0},"number_of_episodes":count,"local_number_of_episodes":count,"local_number_of_seasons":if f.kind=="TV"{1}else{0},
        "can_play":u8::from(view["available"]==true && !matches!(kind,"TV"|"Season")),"play_error":text(&view,"unavailable_reason"),
        "parent_guid":parent,"grand_guid":if f.kind=="TV"{f.id.as_str()}else{""},"ancestor_guid":library(&f.kind),"ancestor_name":if f.kind=="TV"{"剧集"}else{"电影"},
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
fn quality_label(info: &probe::MediaInfo) -> String {
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
fn line_label(e: &Episode) -> String {
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

async fn stream_list(app: &App, who: &Identity, reference: &str, refresh: bool) -> Result<Value> {
    let e = resolve_episode(app, who, reference).await?;
    let (_, current, _) = catalog::available(app, &who.user.id, &e.id).await?;
    // 原生列表一次返回。限制整个发现与识别的时间，慢源不能无限拖住播放界面。
    let deadline = Instant::now() + Duration::from_secs(8);
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
    let mut info: HashMap<_, _> = choices
        .iter()
        .map(|(id, episode)| (id.clone(), probe::cached(episode).unwrap_or_default()))
        .collect();
    // 缓存命中立即显示；剩余来源共享后台探测上限，给实际播放保留容量。
    {
        let probes = stream::iter(choices.values().cloned().map(|episode| async move {
            let measured = probe::source_quality(app, &who.user.id, &episode, refresh, false).await;
            (episode, measured)
        }))
        .buffer_unordered(4);
        tokio::pin!(probes);
        while let Ok(Some((episode, measured))) = timeout_at(deadline, probes.next()).await {
            if let Ok(measured) = measured {
                info.insert(episode.id.clone(), measured);
            }
        }
    }
    catalog::available(app, &who.user.id, &e.id).await?;
    let mut files = Vec::new();
    let mut videos = Vec::new();
    let mut audios = Vec::new();
    let mut subtitles = Vec::new();
    let mut choices: Vec<_> = choices.into_values().collect();
    choices.sort_by(|a, b| {
        quality_rank(&info[&b.id])
            .cmp(&quality_rank(&info[&a.id]))
            .then(a.id.cmp(&b.id))
    });
    for episode in choices {
        let measured = &info[&episode.id];
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
async fn source_season_row(
    app: &App,
    who: &Identity,
    f: &Film,
    reference: &Episode,
    info: &probe::MediaInfo,
) -> Result<Value> {
    let line = &reference.line_name;
    let episodes: Vec<_> = catalog::episodes(app, &f.id)
        .await?
        .into_iter()
        .filter(|e| &e.line_name == line)
        .collect();
    let first = episodes.first().ok_or_else(Error::missing)?;
    catalog::available(app, &who.user.id, &first.id).await?;
    let source = catalog::source(app, &f.source_id).await?;
    let label = format!(
        "{} · {} · {}：{}",
        source.name,
        line_label(first),
        reference.title,
        quality_label(info)
    );
    let mut value = row(app, who, f, None, true).await?;
    value["guid"] = json!(source_season(app, &f.id, line, &reference.id).await?);
    value["title"] = json!(label);
    value["overview"] = json!(format!("播放版本：{label}\n{}", f.overview));
    value["source_name"] = json!(source.name);
    value["source_id"] = json!(source.id);
    value["quality"] = probe::quality(info);
    let poster_version = hash(format!(
        "{}|{}|{}",
        source.name,
        reference.title,
        quality_label(info)
    ));
    let poster = format!(
        "/fntv-source-poster/{}/{}.png",
        text(&value, "guid"),
        &poster_version[..16]
    );
    value["poster"] = json!(poster);
    value["posters"] = json!(poster);
    value["poster_list"] = json!([poster]);
    value["play_item_guid"] = json!(reference.id);
    value["media_guid"] = json!(reference.id);
    value["number_of_episodes"] = json!(episodes.len());
    value["local_number_of_episodes"] = json!(episodes.len());
    value["media_stream"]["resolutions"] = json!(if native_resolution(info).is_empty() {
        vec![]
    } else {
        vec![native_resolution(info)]
    });
    catalog::available(app, &who.user.id, &reference.id).await?;
    Ok(value)
}
async fn source_poster(app: &App, who: &Identity, path: &str) -> Result<Response> {
    let id = path.split('/').next().ok_or_else(Error::missing)?;
    let view = source_season_info(app, id)
        .await?
        .ok_or_else(Error::missing)?;
    let (e, f, source) = catalog::available(app, &who.user.id, &text(&view, "episode_id")).await?;
    if e.film_id != text(&view, "film_id") || e.line_name != text(&view, "line_name") {
        return Err(Error::missing());
    }
    let line = line_label(&e);
    let quality = quality_label(&probe::cached(&e).unwrap_or_default());
    let episode_id = e.id;
    let image = tokio::task::spawn_blocking(move || {
        crate::source_poster::render(&source.name, &line, &e.title, &quality, &f.title)
    })
    .await
    .map_err(|_| Error::bad("来源标签生成失败"))??;
    catalog::available(app, &who.user.id, &episode_id).await?;
    Ok(Response::builder()
        .header("content-type", "image/png")
        .header("cache-control", "private, no-store")
        .body(Body::from(image))
        .unwrap())
}
async fn source_seasons(app: &App, who: &Identity, id: &str) -> Result<Vec<Value>> {
    let original = resolve_episode(app, who, id).await?;
    let versions = stream_list(app, who, &original.id, false).await?;
    let mut rows = Vec::new();
    for file in versions["files"].as_array().into_iter().flatten() {
        let episode_id = text(file, "guid");
        let (e, f, _) = match catalog::available(app, &who.user.id, &episode_id).await {
            Ok(available) => available,
            Err(error) if episode_id == original.id => return Err(error),
            Err(_) => continue,
        };
        let info = probe::cached(&e).unwrap_or_default();
        match source_season_row(app, who, &f, &e, &info).await {
            Ok(row) => rows.push(row),
            Err(error) if e.id == original.id => return Err(error),
            Err(_) => {}
        }
    }
    catalog::available(app, &who.user.id, &original.id).await?;
    let allowed: HashSet<_> = access::sources(app, &who.user.id)
        .await?
        .into_iter()
        .filter(|source| source.active())
        .map(|source| source.id)
        .collect();
    rows.retain(|row| allowed.contains(&text(row, "source_id")));
    Ok(rows)
}

async fn validate_item_media(app: &App, who: &Identity, item: &str, e: &Episode) -> Result<()> {
    if item.is_empty() || item == e.id || item == e.film_id || item == season(&e.film_id) {
        return Ok(());
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
async fn finish_source_switch(app: &App, who: &Identity, f: &Film, play_id: &str) -> Result<()> {
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
    if !streams.iter().any(|v| v["codec_type"] == "video") {
        streams.push(probe::video(info));
    }
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subtitle = Vec::new();
    for mut v in streams {
        let index = v["index"].as_i64().unwrap_or(0);
        let kind = text(&v, "codec_type");
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
                    "hls"
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
        for v in result["items"].as_array().into_iter().flatten() {
            let f = catalog::film(app, &text(v, "id")).await?;
            rows.push(row(app, who, &f, None, false).await?);
        }
    } else if !ancestor.is_empty() && ancestor != library("Movie") && ancestor != library("TV") {
        let f = authorized_film(app, who, &ancestor).await?;
        let f = catalog::detail(app, &who.user.id, &f.id, false).await?;
        if ancestor == f.id {
            if f.kind == "TV" {
                rows.extend(source_seasons(app, who, &f.id).await?);
            } else {
                rows.push(row(app, who, &f, None, true).await?);
            }
        } else {
            let eps = selected_line_episodes(app, who, &ancestor, &f.id).await?;
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
        for id in ids {
            let f = catalog::film(app, &id).await?;
            if ancestor.is_empty() || ancestor == library(&f.kind) {
                rows.push(row(app, who, &f, None, false).await?);
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
        rows.sort_by_key(|a| text(a, "title"));
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
                for v in result["items"].as_array().into_iter().flatten() {
                    let f = catalog::film(app, &text(v, "id")).await?;
                    rows.push(row(app, &who, &f, None, false).await?);
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
                return Ok(ok(source_season_row(
                    app,
                    &who,
                    &f,
                    &e,
                    &probe::cached(&e).unwrap_or_default(),
                )
                .await?));
            }
            let e = catalog::episode(app, id).await.ok();
            return Ok(ok(
                row(app, &who, &f, e.as_ref(), id == season(&f.id)).await?
            ));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/season/list/") {
            let f = catalog::detail(
                app,
                &who.user.id,
                &authorized_film(app, &who, id).await?.id,
                false,
            )
            .await?;
            if f.kind == "TV" {
                return Ok(ok(json!(source_seasons(app, &who, id).await?)));
            }
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
            let eps = selected_line_episodes(app, &who, id, &f.id).await?;
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
            catalog::available(app, &who.user.id, &e.id).await?;
            let p = playback::latest(app, &who, &e.id).await?;
            if p.mode == "direct" {
                playback::event(
                    app,
                    &p,
                    "legacy_media_requested",
                    "客户端请求文件传输入口；请启用播放器直链模式，或在播放日志选择转发重试",
                )
                .await?;
                return Err(Error::bad(
                    "请使用飞牛直链画质；需要转发时先在本项目播放日志选择转发重试",
                ));
            }
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
            let p:Option<playback::Play>=sqlx::query_as("SELECT * FROM plays WHERE auth_hash=? AND native_link_hash=? ORDER BY seq DESC LIMIT 1").bind(&who.auth_hash).bind(hash(link)).fetch_optional(&app.db).await?;
            if let Some(p) = p {
                playback::end(app, &p).await?;
            }
            return Ok(ok(json!(true)));
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
        let f = resolve_film(app, reference).await?;
        catalog::favorite(app, &who.user.id, &f.id, method == Method::PUT).await?;
        return Ok(ok(json!(true)));
    }
    if path == "/v/api/v1/item/watched" {
        authorized_film(app, &who, reference).await?;
        return Ok(ok(json!(true)));
    }
    let e = resolve_episode(app, &who, reference).await?;
    validate_item_media(app, &who, &item, &e).await?;
    catalog::available(app, &who.user.id, &e.id).await?;
    let f = catalog::film(app, &e.film_id).await?;
    let info = probe::get(app, &e).await?;
    catalog::available(app, &who.user.id, &e.id).await?;
    let source = catalog::source(app, &f.source_id).await?;
    let (video, audio, subtitle) = tracks(&e, &info, &source.name);
    match path.as_str() {
        "/v/api/v1/play/info" => {
            let prior: Option<(String, f64)> = sqlx::query_as(
                "SELECT episode_id,position FROM progress WHERE user_id=? AND film_id=?",
            )
            .bind(&who.user.id)
            .bind(&f.id)
            .fetch_optional(&app.db)
            .await?;
            let mut ts = 0.0;
            if let Some((previous, position)) = prior {
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
                }
            }
            if info.duration > 0.0 {
                ts = ts.min(info.duration);
            }
            let mut item_row = row(app, &who, &f, Some(&e), false).await?;
            let parent = if f.kind == "TV" {
                source_season(app, &f.id, &e.line_name, &e.id).await?
            } else {
                library("Movie")
            };
            item_row["parent_guid"] = json!(parent);
            Ok(ok(
                json!({"guid":e.id,"grand_guid":if f.kind=="TV"{f.id.clone()}else{String::new()},"parent_guid":parent,"type":if f.kind=="TV"{"Episode"}else{"Movie"},
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
            let previous = playback::latest(app, &who, &e.id)
                .await
                .ok()
                .filter(|p| p.mode == "proxy" && p.state == "fallback");
            let (delivered, play_id) = if let Some(ref p) = previous {
                playback::proxy_allowed(app, p).await?;
                let access = random();
                sqlx::query("UPDATE plays SET access_hash=? WHERE id=?")
                    .bind(hash(&access))
                    .bind(&p.id)
                    .execute(&app.db)
                    .await?;
                let asset = playback::asset(app, p, &e.url).await?;
                let base = if !app.public_url.is_empty() {
                    app.public_url.clone()
                } else {
                    let host = headers
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .ok_or_else(|| Error::bad("缺少服务地址"))?;
                    format!("http://{host}")
                };
                playback::event(app, p, "proxy_issued", "已为飞牛当前播放交付转发地址").await?;
                (
                    format!("{}/media/{access}/{asset}", base.trim_end_matches('/')),
                    p.id.clone(),
                )
            } else {
                let started = playback::start(app, &who, &e.id).await?;
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
            Ok(ok(
                json!({"file_stream":source_file(app,&who.user.id,&e,&info).await?,"video_stream":video.first(),"audio_streams":audio,"subtitle_streams":subtitle,
                "qualities":[],"direct_link_qualities":[{"bitrate":probe::quality(&info)["bitrate"].as_f64().unwrap_or(0.0) as u64,"resolution":quality_label(&info),"progressive":false,"is_m3u8":info.kind=="hls","url":delivered}],
                "quality":probe::quality(&info),"source_name":source.name,
                "header":{},"direct_link_audio_streams":[],"cloud_storage_info":{"cloud_nick_name":source.name,"is_vip":false,"cloud_storage_type":9001}}),
            ))
        }
        "/v/api/v1/play/play" => {
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
