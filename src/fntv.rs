//! 飞牛播放器协议适配。兼容版本字段仅用于协商，不表示本项目是 fnOS。
use crate::{
    access, auth,
    catalog::{self, Episode, Film},
    core::*,
    net, playback, probe,
};
use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::Method,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

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
    let e = catalog::episodes(app, &f.id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::bad("该影片没有可识别的播放地址"))?;
    Ok(catalog::available(app, &who.user.id, &e.id).await?.0)
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
    let video = probe::video(&info);
    let height = video["height"].as_u64().unwrap_or(0);
    Ok(
        json!({"guid":id,"title":title,"type":kind,"lan":"zh-CN","tv_title":f.title,"parent_title":text(&view,"source_name"),
        "original_title":f.title,"imdb_id":"","trim_id":"","poster":poster,"posters":poster,"backdrops":"","vote_average":"0","genres":[],
        "release_date":f.year,"overview":format!("来源：{}\n{}",text(&view,"source_name"),text(&view,"overview")),"is_favorite":u8::from(view["favorite"]==true),
        "is_watched":0,"watched":0,"watched_ts":position as u64,"season_number":1,"episode_number":episode.map(|e|e.number).unwrap_or(0),
        "number_of_seasons":if f.kind=="TV"{1}else{0},"number_of_episodes":count,"local_number_of_episodes":count,"local_number_of_seasons":if f.kind=="TV"{1}else{0},
        "can_play":u8::from(view["available"]==true && !matches!(kind,"TV"|"Season")),"play_error":text(&view,"unavailable_reason"),
        "parent_guid":parent,"grand_guid":if f.kind=="TV"{f.id.as_str()}else{""},"ancestor_guid":library(&f.kind),"ancestor_name":if f.kind=="TV"{"剧集"}else{"电影"},
        "ancestor_category":f.kind,"play_item_guid":ep.map(|e|e.id.as_str()).unwrap_or(""),"media_guid":ep.map(|e|e.id.as_str()).unwrap_or(""),
        "duration":info.duration as u64,"logic_type":0,"single_child_guid":"","file_name":ep.map(|e|format!("{}.{}",e.title,if info.kind=="hls"{"m3u8"}else{"mp4"})).unwrap_or_default(),
        "create_time":f.updated_at*1000,"update_time":f.updated_at*1000,"first_air_date":"","last_air_date":"","air_date":"","status":"","douban_id":0,
        "poster_list":if poster.is_empty(){vec![]}else{vec![poster]},"media_stream":{"resolutions":if height>0{vec![height.to_string()]}else{vec![]},"audio_type":null,"color_range_type":[]}}),
    )
}
fn file(e: &Episode, info: &probe::MediaInfo) -> Value {
    json!({"guid":e.id,"path":format!("/tv-rust/{}.{}",e.id,if info.kind=="hls"{"m3u8"}else{"mp4"}),"file_name":format!("{}.{}",e.title,if info.kind=="hls"{"m3u8"}else{"mp4"}),
        "size":info.size,"timestamp":now()*1000,"type":1,"can_play":1,"play_error":"","create_time":now()*1000,"update_time":now()*1000,"file_birth_time":now()*1000,"progress_thumb_hash_dir":""})
}
fn tracks(e: &Episode, info: &probe::MediaInfo) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
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
                v["resolution_type"] = json!(
                    v["height"]
                        .as_u64()
                        .filter(|n| *n > 0)
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                );
                v["color_range_type"] = json!("SDR");
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
            rows.push(row(app, who, &f, None, true).await?);
        } else {
            let eps = catalog::episodes(app, &f.id).await?;
            let first = eps.first().map(|e| e.line_order).unwrap_or(0);
            for e in eps.iter().filter(|e| e.line_order == first) {
                rows.push(row(app, who, &f, Some(e), false).await?);
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
                let ids:Vec<String>=sqlx::query_scalar("SELECT episode_id FROM progress WHERE user_id=? ORDER BY updated_at DESC LIMIT 100").bind(&who.user.id).fetch_all(&app.db).await?;
                let mut rows = Vec::new();
                for id in ids {
                    let e = catalog::episode(app, &id).await?;
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
            let eps = catalog::episodes(app, &f.id).await?;
            let line = eps.first().map(|e| e.line_order).unwrap_or(0);
            let mut rows = Vec::new();
            for e in eps.iter().filter(|e| e.line_order == line) {
                rows.push(row(app, &who, &f, Some(e), false).await?);
            }
            return Ok(ok(json!(rows)));
        }
        if let Some(id) = path.strip_prefix("/v/api/v1/stream/list/") {
            let e = resolve_episode(app, &who, id).await?;
            let info = probe::get(app, &e).await?;
            catalog::available(app, &who.user.id, &e.id).await?;
            let (video, audio, subtitle) = tracks(&e, &info);
            return Ok(ok(
                json!({"files":[file(&e,&info)],"video_streams":video,"audio_streams":audio,"subtitle_streams":subtitle}),
            ));
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
    catalog::available(app, &who.user.id, &e.id).await?;
    let f = catalog::film(app, &e.film_id).await?;
    let info = probe::get(app, &e).await?;
    catalog::available(app, &who.user.id, &e.id).await?;
    let (video, audio, subtitle) = tracks(&e, &info);
    match path.as_str() {
        "/v/api/v1/play/info" => {
            let prior: Option<(String, f64)> = sqlx::query_as(
                "SELECT episode_id,position FROM progress WHERE user_id=? AND film_id=?",
            )
            .bind(&who.user.id)
            .bind(&f.id)
            .fetch_optional(&app.db)
            .await?;
            let ts = prior.filter(|p| p.0 == e.id).map(|p| p.1).unwrap_or(0.0);
            Ok(ok(
                json!({"guid":e.id,"grand_guid":if f.kind=="TV"{f.id.clone()}else{String::new()},"parent_guid":if f.kind=="TV"{season(&f.id)}else{library("Movie")},"type":if f.kind=="TV"{"Episode"}else{"Movie"},
                "media_guid":e.id,"video_guid":video.first().map(|v|text(v,"guid")).unwrap_or_default(),"audio_guid":audio.first().map(|v|text(v,"guid")).unwrap_or_default(),"subtitle_guid":"","direct_link_audio_index":0,"ts":ts as u64,
                "play_config":{"skip_opening":null,"skip_ending":null},"item":row(app,&who,&f,Some(&e),false).await?}),
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
            Ok(ok(
                json!({"file_stream":file(&e,&info),"video_stream":video.first(),"audio_streams":audio,"subtitle_streams":subtitle,
                "qualities":[],"direct_link_qualities":[{"bitrate":0,"resolution":"原画","progressive":false,"is_m3u8":info.kind=="hls","url":delivered}],
                "header":{},"direct_link_audio_streams":[],"cloud_storage_info":{"cloud_nick_name":"视频源","is_vip":false,"cloud_storage_type":9001}}),
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
            if !item.is_empty() && item != e.id && item != f.id {
                return Err(Error::bad("观看记录的影片与媒体不匹配"));
            }
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
