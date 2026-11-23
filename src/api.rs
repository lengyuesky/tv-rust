use crate::{access, auth, catalog, core::*, net, playback, preferences};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::HashMap;

pub async fn handle(State(app): State<App>, request: Request) -> Response {
    inner(&app, request)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}
fn json_response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
fn cookie(app: &App, token: &str, remove: bool) -> String {
    format!(
        "tv_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        if remove { 0 } else { 30 * 86400 },
        if app.public_url.starts_with("https://") {
            "; Secure"
        } else {
            ""
        }
    )
}
pub async fn poster(app: &App, user: &str, id: &str) -> Result<Response> {
    let f = catalog::film(app, id).await?;
    access::require_source(app, user, &f.source_id).await?;
    if f.poster.is_empty() {
        return Err(Error::missing());
    }
    let r = net::request(app, Method::GET, &f.poster, &HeaderMap::new()).await?;
    if !r.status().is_success() {
        return Err(Error::missing());
    }
    let mime = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_owned();
    if !matches!(
        mime.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/avif" | "image/gif"
    ) {
        return Err(Error::bad("封面格式不受支持"));
    }
    let bytes = net::bytes(r, 5 * 1024 * 1024, false).await?;
    access::require_source(app, user, &f.source_id).await?;
    Ok(Response::builder()
        .header("content-type", mime)
        .header("cache-control", "private, max-age=3600")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .unwrap())
}
async fn inner(app: &App, request: Request) -> Result<Response> {
    let path = request.uri().path().trim_end_matches('/').to_owned();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if path == "/api/health" {
        return Ok(json_response(
            json!({"ok":true,"version":env!("CARGO_PKG_VERSION"),"build_revision":build_revision()}),
        ));
    }
    if path == "/api/status" {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&app.db)
            .await?;
        return Ok(json_response(
            json!({"initialized":count>0,"version":env!("CARGO_PKG_VERSION")}),
        ));
    }
    let mut value = json!({});
    if !matches!(method, Method::GET | Method::HEAD) {
        if headers.get("x-tv-request").and_then(|v| v.to_str().ok()) != Some("1") {
            return Err(Error::denied("缺少同源请求标记"));
        }
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
            let host = headers
                .get("host")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let origin = url::Url::parse(origin).map_err(|_| Error::denied("请求来源无效"))?;
            let authority = origin[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
            if authority != host
                && origin.origin().ascii_serialization() != app.public_url.trim_end_matches('/')
            {
                return Err(Error::denied("不允许跨站修改"));
            }
        }
        let bytes = to_bytes(request.into_body(), 4 * 1024 * 1024)
            .await
            .map_err(|_| Error::bad("请求体过大"))?;
        if !bytes.is_empty() {
            value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("请求不是有效 JSON"))?;
        }
    }
    if path == "/api/setup" && method == Method::POST {
        auth::setup(app, &value).await?;
        return Ok(json_response(json!({"ok":true})));
    }
    if path == "/api/login" && method == Method::POST {
        let (token, user) = auth::login(
            app,
            &text(&value, "username"),
            &text(&value, "password"),
            false,
        )
        .await?;
        let mut r = json_response(json!({"user":user}));
        r.headers_mut()
            .insert("set-cookie", cookie(app, &token, false).parse().unwrap());
        return Ok(r);
    }
    let who = auth::identity(app, &headers).await?;
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/me") => {
            return Ok(json_response(
                json!({"preferences":preferences::get(app,&who.user.id).await?,"user":who.user,"forbid_proxy":*app.forbid_proxy.borrow(),"public_url":app.public_url,"version":env!("CARGO_PKG_VERSION"),"build_revision":build_revision()}),
            ));
        }
        ("PATCH", "/api/me/preferences") => {
            return Ok(json_response(
                json!({"preferences":preferences::save(app,&who.user.id,&value).await?}),
            ));
        }
        ("POST", "/api/logout") => {
            sqlx::query("DELETE FROM auth_sessions WHERE token_hash=?")
                .bind(&who.auth_hash)
                .execute(&app.db)
                .await?;
            let mut r = json_response(json!({"ok":true}));
            r.headers_mut()
                .insert("set-cookie", cookie(app, "", true).parse().unwrap());
            return Ok(r);
        }
        ("POST", "/api/password") => {
            auth::change_password(app, &who, &value).await?;
            return Ok(json_response(json!({"ok":true})));
        }
        ("GET", "/api/users") => return Ok(json_response(auth::users(app, &who).await?)),
        ("POST", "/api/users") => {
            auth::create_user(app, &who, &value).await?;
            return Ok(json_response(json!({"ok":true})));
        }
        ("GET", "/api/subscriptions") => {
            who.admin()?;
            return Ok(json_response(catalog::subscription_list(app).await?));
        }
        ("POST", "/api/subscriptions") => {
            who.admin()?;
            return Ok(json_response(
                json!({"id":catalog::import(app,&value).await?}),
            ));
        }
        ("GET", "/api/sources") => {
            return Ok(json_response(
                json!({"sources":access::sources(app,&who.user.id).await?.into_iter().filter(|s|s.active()).map(|s|json!({"id":s.id,"name":s.name})).collect::<Vec<_>>()}),
            ));
        }
        ("GET", "/api/search") => {
            return Ok(json_response(
                catalog::search(
                    app,
                    &who.user.id,
                    query.get("q").map(String::as_str).unwrap_or(""),
                    query.get("page").and_then(|v| v.parse().ok()).unwrap_or(1),
                    query
                        .get("source")
                        .filter(|s| !s.is_empty())
                        .map(String::as_str),
                )
                .await?,
            ));
        }
        ("GET", "/api/recommendations") => {
            return Ok(json_response(
                catalog::recommendations(
                    app,
                    query.get("kind").map(String::as_str).unwrap_or("movie"),
                )
                .await?,
            ));
        }
        ("GET", "/api/favorites") => {
            return Ok(json_response(
                json!({"items":catalog::personal(app,&who.user.id,"favorites").await?}),
            ));
        }
        ("GET", "/api/history") => {
            return Ok(json_response(
                json!({"items":catalog::history(app,&who.user.id).await?}),
            ));
        }
        ("GET", "/api/continue-watching") => {
            return Ok(json_response(
                json!({"items":catalog::continue_watching(app,&who.user.id).await?}),
            ));
        }
        ("GET", "/api/logs") => {
            return Ok(json_response(
                playback::logs(
                    app,
                    &who,
                    query
                        .get("before")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                )
                .await?,
            ));
        }
        ("GET", "/api/settings") => {
            return Ok(json_response(
                json!({"forbid_proxy":*app.forbid_proxy.borrow(),"public_url":app.public_url}),
            ));
        }
        ("PATCH", "/api/settings") => {
            who.admin()?;
            let on = value["forbid_proxy"]
                .as_bool()
                .ok_or_else(|| Error::bad("禁止转发设置必须为布尔值"))?;
            playback::set_forbidden(app, on).await?;
            return Ok(json_response(json!({"forbid_proxy":on})));
        }
        ("POST", "/api/plays") => {
            return Ok(json_response(
                playback::start(app, &who, &text(&value, "episode_id")).await?,
            ));
        }
        ("GET", "/api/backup") => {
            who.admin()?;
            let dir = app.data_dir.join("backups");
            tokio::fs::create_dir_all(&dir).await?;
            let name = format!("tv-rust-{}-{}.sqlite", now(), &random()[..8]);
            let target = dir.join(&name);
            sqlx::query("VACUUM INTO ?")
                .bind(target.to_string_lossy().as_ref())
                .execute(&app.db)
                .await?;
            let data = tokio::fs::read(&target).await?;
            tokio::fs::remove_file(target).await?;
            return Ok(Response::builder()
                .header("content-type", "application/vnd.sqlite3")
                .header(
                    "content-disposition",
                    format!("attachment; filename=\"{name}\""),
                )
                .header("cache-control", "no-store")
                .body(Body::from(data))
                .unwrap());
        }
        _ => {}
    }
    let parts = path
        .trim_start_matches("/api/")
        .split('/')
        .collect::<Vec<_>>();
    match parts.as_slice() {
        ["films", id, "source-quality"] if method == Method::GET => {
            return Ok(json_response(
                catalog::source_quality(
                    app,
                    &who.user.id,
                    id,
                    query.get("switch_from").map(String::as_str).unwrap_or(""),
                    query
                        .get("switch_episode")
                        .map(String::as_str)
                        .unwrap_or(""),
                    query.get("refresh").is_some_and(|value| value == "1"),
                    query.get("selected").is_some_and(|value| value == "1"),
                )
                .await?,
            ));
        }
        ["films", id, "sources"] if method == Method::GET => {
            let refresh = query.get("refresh").is_some_and(|value| value == "1");
            return Ok(json_response(
                catalog::playback_sources(
                    app,
                    &who.user.id,
                    id,
                    refresh || query.get("discover").is_some_and(|value| value == "1"),
                    refresh,
                )
                .await?,
            ));
        }
        ["films", id, "posters"] if method == Method::GET => {
            return Ok(json_response(
                catalog::film_posters(app, &who.user.id, id).await?,
            ));
        }
        ["recommendations", id, "posters"] if method == Method::GET => {
            return Ok(json_response(
                catalog::recommendation_posters(
                    app,
                    &who.user.id,
                    id,
                    query.get("kind").map(String::as_str).unwrap_or("movie"),
                )
                .await?,
            ));
        }
        ["users", id] if method == Method::DELETE => {
            auth::delete_user(app, &who, id).await?;
        }
        ["users", id] if method == Method::PATCH => {
            who.admin()?;
            if *id == who.user.id {
                return Err(Error::bad("不能停用当前管理员"));
            }
            let disabled = value["disabled"]
                .as_bool()
                .ok_or_else(|| Error::bad("账号状态无效"))?;
            sqlx::query("UPDATE users SET disabled=? WHERE id=?")
                .bind(disabled)
                .bind(id)
                .execute(&app.db)
                .await?;
            if disabled {
                sqlx::query("DELETE FROM auth_sessions WHERE user_id=?")
                    .bind(id)
                    .execute(&app.db)
                    .await?;
            }
        }
        ["subscriptions", id, "refresh"] if method == Method::POST => {
            who.admin()?;
            catalog::refresh(app, id).await?;
        }
        ["subscriptions", id] if method == Method::PATCH => {
            who.admin()?;
            return Ok(json_response(
                json!({"usage":access::save_usage(app,id,&value["usage"]).await?}),
            ));
        }
        ["subscriptions", id] if method == Method::DELETE => {
            who.admin()?;
            catalog::remove_subscription(app, id).await?;
        }
        ["sources", id] if method == Method::PATCH => {
            who.admin()?;
            let enabled = value["enabled"]
                .as_bool()
                .ok_or_else(|| Error::bad("源状态无效"))?;
            sqlx::query("UPDATE sources SET enabled=? WHERE id=?")
                .bind(enabled)
                .bind(id)
                .execute(&app.db)
                .await?;
        }
        ["films", id] if method == Method::GET => {
            let f = catalog::film(app, id).await?;
            access::require_source(app, &who.user.id, &f.source_id).await?;
            let force = query.get("refresh").is_some_and(|v| v == "1");
            let mut warning = String::new();
            if let Err(e) = catalog::detail(app, &who.user.id, id, force).await {
                warning = e.1;
            }
            let mut v = catalog::view(app, &who.user.id, id, true).await?;
            v["warning"] = json!(warning);
            if let Some(from) = query.get("switch_from").filter(|s| !s.is_empty()) {
                v["switch_hint"] = if let Some(episode) =
                    query.get("switch_episode").filter(|s| !s.is_empty())
                {
                    let position: Option<f64> = sqlx::query_scalar("SELECT position FROM progress WHERE user_id=? AND film_id=? AND episode_id=?")
                            .bind(&who.user.id).bind(from).bind(episode).fetch_optional(&app.db).await?;
                    catalog::switch_episode_hint(
                        app,
                        &who.user.id,
                        id,
                        from,
                        episode,
                        query.get("switch_line").map(String::as_str),
                        position.unwrap_or(0.0),
                    )
                    .await?
                } else {
                    catalog::switch_hint(app, &who.user.id, id, from).await?
                };
            }
            return Ok(json_response(v));
        }
        ["films", id, "favorite"] if matches!(method, Method::PUT | Method::DELETE) => {
            catalog::favorite(app, &who.user.id, id, method == Method::PUT).await?;
        }
        ["films", id, "history"] if method == Method::DELETE => {
            playback::remove_history(app, &who.user.id, &[(*id).to_owned()]).await?;
        }
        ["posters", id] if method == Method::GET => return poster(app, &who.user.id, id).await,
        ["plays", id, "fallback"] if method == Method::POST => {
            return Ok(json_response(
                playback::fallback(app, &who, id, &text(&value, "reason")).await?,
            ));
        }
        ["plays", id, "progress"] if method == Method::POST => {
            let p = playback::owned(app, &who, id).await?;
            let saved = playback::record(
                app,
                &p,
                value["position"]
                    .as_f64()
                    .ok_or_else(|| Error::bad("缺少观看位置"))?,
                value["duration"].as_f64().unwrap_or(0.0),
                None,
            )
            .await?;
            return Ok(json_response(json!({"saved":saved})));
        }
        ["plays", id, "event"] if method == Method::POST => {
            let p = playback::owned(app, &who, id).await?;
            catalog::available(app, &who.user.id, &p.episode_id).await?;
            let event = text(&value, "event");
            let msg = match event.as_str() {
                "playing" => "网页播放器报告开始播放",
                "waiting" => "网页播放器报告等待数据",
                "error" => "网页播放器报告播放失败",
                "proxy_blocked" => "禁止转发已开启，客户端未尝试回退",
                _ => return Err(Error::bad("日志事件类型无效")),
            };
            playback::event(app, &p, &event, msg).await?;
        }
        ["plays", id, "end"] if method == Method::POST => {
            let p = playback::owned(app, &who, id).await?;
            // 结束请求携带最终快照，窗口关闭时也可一次保存并结束，避免两个请求乱序。
            let snapshot = if value.get("position").is_some() {
                Some((
                    value["position"]
                        .as_f64()
                        .ok_or_else(|| Error::bad("观看位置无效"))?,
                    value["duration"]
                        .as_f64()
                        .ok_or_else(|| Error::bad("媒体时长无效"))?,
                ))
            } else {
                None
            };
            playback::end_with_snapshot(app, &p, snapshot).await?;
        }
        _ => return Err(Error::missing()),
    }
    Ok(json_response(json!({"ok":true})))
}
