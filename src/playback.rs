use crate::{access, catalog, core::*, net, probe};
use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Play {
    pub seq: i64,
    pub id: String,
    pub access_hash: String,
    pub user_id: String,
    pub auth_hash: String,
    pub film_id: String,
    pub episode_id: String,
    pub client: String,
    pub mode: String,
    pub reason: String,
    pub state: String,
    pub started_at: i64,
    pub expires_at: i64,
    pub position: f64,
    pub reports: i64,
    pub native_link_hash: String,
    pub ended: bool,
    pub recording_disabled: bool,
}
pub async fn event(app: &App, p: &Play, kind: &str, message: &str) -> Result<()> {
    sqlx::query("INSERT INTO play_events(play_id,at,mode,event,message) VALUES(?,?,?,?,?)")
        .bind(&p.id)
        .bind(now())
        .bind(&p.mode)
        .bind(kind)
        .bind(message)
        .execute(&app.db)
        .await?;
    tracing::info!(play=%p.id,mode=%p.mode,event=kind,"{message}");
    Ok(())
}
pub async fn get(app: &App, id: &str) -> Result<Play> {
    sqlx::query_as("SELECT * FROM plays WHERE id=?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)
}
pub async fn owned(app: &App, who: &Identity, id: &str) -> Result<Play> {
    let p = get(app, id).await?;
    if p.user_id != who.user.id {
        return Err(Error::denied("无法操作其他用户的播放"));
    }
    Ok(p)
}
pub async fn latest(app: &App, who: &Identity, episode_id: &str) -> Result<Play> {
    sqlx::query_as("SELECT * FROM plays WHERE auth_hash=? AND episode_id=? AND ended=0 AND expires_at>? ORDER BY seq DESC LIMIT 1")
        .bind(&who.auth_hash).bind(episode_id).bind(now()).fetch_optional(&app.db).await?.ok_or_else(||Error::bad("播放会话已结束，请重新播放"))
}
async fn valid(app: &App, p: &Play) -> Result<()> {
    if p.state == "access_revoked" {
        return Err(Error::denied(access::UNAUTHORIZED_SOURCE));
    }
    if p.ended || p.expires_at <= now() {
        return Err(Error::unauthorized());
    }
    let valid:i64=sqlx::query_scalar("SELECT count(*) FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND s.user_id=? AND s.expires_at>? AND u.disabled=0")
        .bind(&p.auth_hash).bind(&p.user_id).bind(now()).fetch_one(&app.db).await?;
    if valid != 1 {
        return Err(Error::unauthorized());
    }
    catalog::available(app, &p.user_id, &p.episode_id).await?;
    Ok(())
}
pub async fn asset(app: &App, p: &Play, url: &str) -> Result<String> {
    let mut resources = BTreeMap::new();
    let id = map_asset(p, url, &mut resources, false)?;
    register_assets(app, p, &resources).await?;
    Ok(id)
}
fn map_asset(
    p: &Play,
    url: &str,
    resources: &mut BTreeMap<String, String>,
    key: bool,
) -> Result<String> {
    let parsed = net::valid_url(url)?;
    // FFmpeg HLS 会检查分片扩展名；仅保留格式后缀，不暴露上游路径和查询凭据。
    let extension = parsed
        .path()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let extension = if key { "key" } else { extension.as_str() };
    let suffix = match extension {
        "m3u8" | "ts" | "m4s" | "mp4" | "m4a" | "aac" | "mp3" | "ac3" | "ec3" | "vtt"
        | "webvtt" | "key" | "bin" => format!(".{extension}"),
        _ => String::new(),
    };
    let id = format!("{}{suffix}", guid(&["asset", &p.id, url]));
    resources
        .entry(id.clone())
        .or_insert_with(|| url.to_owned());
    Ok(id)
}
async fn register_assets(app: &App, p: &Play, resources: &BTreeMap<String, String>) -> Result<()> {
    if resources.is_empty() {
        return Ok(());
    }
    // 长影片的清单可包含数千个分片。集中登记，避免逐条写锁拖慢清单并阻塞进度。
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let existing: HashSet<String> =
        sqlx::query_scalar("SELECT id FROM media_assets WHERE play_id=?")
            .bind(&p.id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let pending: Vec<_> = resources
        .iter()
        .filter(|(id, _)| !existing.contains(*id))
        .collect();
    if existing.len() + pending.len() > 12000 {
        return Err(Error::bad("此播放会话的媒体资源数量超出上限，请重新播放"));
    }
    for chunk in pending.chunks(200) {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "INSERT OR IGNORE INTO media_assets (id,play_id,url) ",
        );
        query.push_values(chunk.iter().copied(), |mut row, (id, url)| {
            row.push_bind(id).push_bind(&p.id).push_bind(url);
        });
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn start(app: &App, who: &Identity, episode_id: &str) -> Result<Value> {
    let (e, f, source) = catalog::available(app, &who.user.id, episode_id).await?;
    let info = probe::get(app, &e).await?;
    let id = guid(&[&random()]);
    let access = random();
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    if !access::source_allowed_on(&mut tx, &who.user.id, &f.source_id).await? {
        return Err(Error::denied(access::UNAUTHORIZED_SOURCE));
    }
    sqlx::query("INSERT INTO plays(id,access_hash,user_id,auth_hash,film_id,episode_id,client,started_at,expires_at) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(&id).bind(hash(&access)).bind(&who.user.id).bind(&who.auth_hash).bind(&f.id).bind(&e.id).bind(&who.client).bind(now()).bind(now()+6*3600).execute(&mut *tx).await?;
    tx.commit().await?;
    let p = get(app, &id).await?;
    let root = asset(app, &p, &e.url).await?;
    event(
        app,
        &p,
        "direct_issued",
        "已提供直链，尚未确认客户端实际播放",
    )
    .await?;
    let prior: Option<(String, f64)> =
        sqlx::query_as("SELECT episode_id,position FROM progress WHERE user_id=? AND film_id=?")
            .bind(&who.user.id)
            .bind(&f.id)
            .fetch_optional(&app.db)
            .await?;
    let position = prior.filter(|v| v.0 == e.id).map(|v| v.1).unwrap_or(0.0);
    valid(app, &get(app, &id).await?).await?;
    Ok(
        json!({"id":id,"episode_id":e.id,"url":e.url,"mode":"direct","kind":info.kind,"duration":info.duration,
        "position":position,"allow_proxy":!*app.forbid_proxy.borrow(),"proxy_url":format!("/media/{access}/{root}"),"warning":info.warning,
        "quality":probe::quality(&info),"source_id":source.id,"source_name":source.name,"line_name":e.line_name,"episode_title":e.title}),
    )
}
pub async fn proxy_allowed(app: &App, p: &Play) -> Result<()> {
    if *app.forbid_proxy.borrow() {
        event(app, p, "proxy_blocked", "禁止转发已开启，已拦截转发请求").await?;
        return Err(Error::denied(
            "管理员已开启禁止转发，只能使用直链；直链不可用时请换源",
        ));
    }
    Ok(())
}
pub async fn fallback(app: &App, who: &Identity, id: &str, reason: &str) -> Result<Value> {
    let p = owned(app, who, id).await?;
    valid(app, &p).await?;
    proxy_allowed(app, &p).await?;
    let reason = match reason {
        "network" => "客户端报告直链网络或跨域错误",
        "headers" => "直接请求无法满足源的访问条件",
        "manual" => "用户主动选择转发重试",
        _ => return Err(Error::bad("不支持的回退原因")),
    };
    let access = random();
    let changed = sqlx::query("UPDATE plays SET mode='proxy',state='fallback',reason=?,access_hash=? WHERE id=? AND ended=0")
        .bind(reason)
        .bind(hash(&access))
        .bind(id)
        .execute(&app.db)
        .await?;
    if changed.rows_affected() == 0 {
        valid(app, &get(app, id).await?).await?;
        return Err(Error::bad("播放会话已结束，请重新播放"));
    }
    let p = get(app, id).await?;
    event(app, &p, "proxy_selected", reason).await?;
    let e = catalog::episode(app, &p.episode_id).await?;
    let root = asset(app, &p, &e.url).await?;
    Ok(json!({"id":id,"url":format!("/media/{access}/{root}"),"mode":"proxy","reason":reason}))
}
pub async fn record(
    app: &App,
    p: &Play,
    position: f64,
    duration: f64,
    native_link: Option<&str>,
) -> Result<bool> {
    validate_position(position, duration)?;
    if p.recording_disabled {
        return Ok(false);
    }
    let f = catalog::film(app, &p.film_id).await?;
    access::require_source(app, &p.user_id, &f.source_id).await?;
    if p.ended || p.expires_at <= now() {
        return Ok(false);
    }
    // 在读取后写入的事务开始时取得写锁，避免 WAL 快照升级造成 SQLITE_BUSY。
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let current: Play = sqlx::query_as("SELECT * FROM plays WHERE id=?")
        .bind(&p.id)
        .fetch_one(&mut *tx)
        .await?;
    if current.ended || current.recording_disabled || current.expires_at <= now() {
        return Ok(false);
    }
    if !access::source_allowed_on(&mut tx, &p.user_id, &f.source_id).await? {
        return Err(Error::denied(access::UNAUTHORIZED_SOURCE));
    }
    if let Some(link) = native_link {
        if link.is_empty() || link.len() > 16384 {
            return Err(Error::bad("播放关联标识无效"));
        }
        let link = hash(link);
        if !current.native_link_hash.is_empty() && current.native_link_hash != link {
            return Ok(false);
        }
        let older: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM plays WHERE auth_hash=? AND native_link_hash=? AND id<>?",
        )
        .bind(&p.auth_hash)
        .bind(&link)
        .bind(&p.id)
        .fetch_one(&mut *tx)
        .await?;
        if older > 0 {
            return Ok(false);
        }
        sqlx::query("UPDATE plays SET native_link_hash=? WHERE id=? AND native_link_hash=''")
            .bind(link)
            .bind(&p.id)
            .execute(&mut *tx)
            .await?;
    }
    let newer: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plays WHERE user_id=? AND film_id=? AND seq>?")
            .bind(&p.user_id)
            .bind(&p.film_id)
            .bind(p.seq)
            .fetch_one(&mut *tx)
            .await?;
    if newer > 0 {
        return Ok(false);
    }
    save_progress(&mut tx, &current, position, duration).await?;
    tx.commit().await?;
    Ok(true)
}

fn validate_position(position: f64, duration: f64) -> Result<()> {
    if !position.is_finite()
        || !duration.is_finite()
        || !(0.0..=30.0 * 86400.0).contains(&position)
        || !(0.0..=30.0 * 86400.0).contains(&duration)
        || (duration > 0.0 && position > duration + 3.0)
    {
        return Err(Error::bad("播放位置或时长无效"));
    }
    Ok(())
}

async fn save_progress(
    connection: &mut sqlx::SqliteConnection,
    p: &Play,
    position: f64,
    duration: f64,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE plays SET position=?,reports=reports+1,state=CASE WHEN state='fallback' THEN state ELSE 'progress' END WHERE id=? AND ended=0 AND recording_disabled=0",
    )
    .bind(position)
    .bind(&p.id)
    .execute(&mut *connection)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(());
    }
    sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?) ON CONFLICT(user_id,film_id) DO UPDATE SET episode_id=excluded.episode_id,position=excluded.position,duration=excluded.duration,authority=excluded.authority,updated_at=excluded.updated_at WHERE excluded.authority>=progress.authority")
        .bind(&p.user_id).bind(&p.film_id).bind(&p.episode_id).bind(position).bind(duration).bind(p.seq).bind(now()).execute(&mut *connection).await?;
    let message = "收到客户端位置上报；该记录本身不保证画面或声音正常";
    if p.reports == 0 {
        sqlx::query("INSERT INTO play_events(play_id,at,mode,event,message) VALUES(?,?,?,'progress_reported',?)")
            .bind(&p.id).bind(now()).bind(&p.mode).bind(message)
            .execute(&mut *connection).await?;
        tracing::info!(play=%p.id,mode=%p.mode,event="progress_reported","{message}");
    }
    Ok(())
}
pub async fn end(app: &App, p: &Play) -> Result<()> {
    end_with_snapshot(app, p, None).await
}
pub async fn end_with_snapshot(app: &App, p: &Play, snapshot: Option<(f64, f64)>) -> Result<()> {
    if let Some((position, duration)) = snapshot {
        validate_position(position, duration)?;
    }
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let current: Play = sqlx::query_as("SELECT * FROM plays WHERE id=?")
        .bind(&p.id)
        .fetch_one(&mut *tx)
        .await?;
    if current.ended {
        return Ok(());
    }
    if let Some((position, duration)) = snapshot.filter(|_| !current.recording_disabled) {
        let newer: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM plays WHERE user_id=? AND film_id=? AND seq>?",
        )
        .bind(&current.user_id)
        .bind(&current.film_id)
        .bind(current.seq)
        .fetch_one(&mut *tx)
        .await?;
        let source: String = sqlx::query_scalar("SELECT source_id FROM films WHERE id=?")
            .bind(&current.film_id)
            .fetch_one(&mut *tx)
            .await?;
        if newer == 0
            && current.expires_at > now()
            && access::source_allowed_on(&mut tx, &current.user_id, &source).await?
        {
            save_progress(&mut tx, &current, position, duration).await?;
        }
    }
    // 最终快照与 ended 在同一写事务中，迟到的窗口关闭前上报没有插入的机会。
    let r = sqlx::query("UPDATE plays SET ended=1,state='ended' WHERE id=? AND ended=0")
        .bind(&current.id)
        .execute(&mut *tx)
        .await?;
    if r.rows_affected() > 0 {
        sqlx::query("INSERT INTO play_events(play_id,at,mode,event,message) VALUES(?,?,?,'ended','收到客户端结束播放通知')")
            .bind(&current.id).bind(now()).bind(&current.mode).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    tracing::info!(play=%current.id,mode=%current.mode,event="ended","收到客户端结束播放通知");
    Ok(())
}
pub async fn remove_history(app: &App, user: &str, film_ids: &[String]) -> Result<()> {
    if film_ids.is_empty() {
        return Ok(());
    }
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut grouped = HashSet::new();
    for id in film_ids {
        grouped.extend(catalog::history_film_ids_on(&mut tx, user, id).await?);
    }
    let mut menus = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "UPDATE fntv_playback_menus SET recording_disabled=1 WHERE user_id=",
    );
    menus
        .push_bind(user)
        .push(" AND reference_episode_id IN (SELECT id FROM episodes WHERE film_id IN (");
    let mut ids = menus.separated(",");
    for id in &grouped {
        ids.push_bind(id);
    }
    menus.push("))");
    menus.build().execute(&mut *tx).await?;
    // 清除记录和禁用旧会话的进度写入共用一把写锁；正在播放的媒体仍可继续。
    for sql in [
        "UPDATE plays SET recording_disabled=1 WHERE user_id=",
        "DELETE FROM progress WHERE user_id=",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(sql);
        query.push_bind(user).push(" AND film_id IN (");
        let mut ids = query.separated(",");
        for id in &grouped {
            ids.push_bind(id);
        }
        query.push(")");
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn set_forbidden(app: &App, on: bool) -> Result<()> {
    sqlx::query("UPDATE settings SET value=? WHERE key='forbid_proxy'")
        .bind(if on { "true" } else { "false" })
        .execute(&app.db)
        .await?;
    app.forbid_proxy.send_replace(on);
    if on {
        let active: Vec<Play> =
            sqlx::query_as("SELECT * FROM plays WHERE mode='proxy' AND ended=0 AND expires_at>?")
                .bind(now())
                .fetch_all(&app.db)
                .await?;
        for p in active {
            event(
                app,
                &p,
                "proxy_blocked",
                "管理员开启禁止转发，现有媒体转发已撤销",
            )
            .await?;
        }
    }
    Ok(())
}
pub async fn logs(app: &App, who: &Identity, before: i64) -> Result<Value> {
    let rows: Vec<Play> = sqlx::query_as(
        "SELECT * FROM plays WHERE (? OR user_id=?) AND (?=0 OR seq<?) ORDER BY seq DESC LIMIT 100",
    )
    .bind(who.user.admin)
    .bind(&who.user.id)
    .bind(before)
    .bind(before)
    .fetch_all(&app.db)
    .await?;
    let mut items = Vec::new();
    for p in rows {
        let f = catalog::film(app, &p.film_id).await?;
        let s = catalog::source(app, &f.source_id).await?;
        let e = catalog::episode(app, &p.episode_id).await?;
        let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id=?")
            .bind(&p.user_id)
            .fetch_one(&app.db)
            .await?;
        let events: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT at,mode,event,message FROM play_events WHERE play_id=? ORDER BY id",
        )
        .bind(&p.id)
        .fetch_all(&app.db)
        .await?;
        let authorized = access::source_allowed(app, &who.user.id, &s.id).await?;
        items.push(json!({"seq":p.seq,"id":p.id,"username":username,"client":p.client,"film_id":f.id,"title":f.title,"episode":e.title,"source":s.name,"mode":p.mode,"state":p.state,
            "started_at":p.started_at,"reason":p.reason,"position":p.position,"reports":p.reports,"ended":p.ended,"mine":p.user_id==who.user.id,
            "authorized":authorized,"unavailable_reason":if authorized {""} else {access::UNAUTHORIZED_SOURCE},
            "events":events.into_iter().map(|(at,mode,event,message)|json!({"at":at,"mode":mode,"event":event,"message":message})).collect::<Vec<_>>()}));
    }
    Ok(json!({"items":items,"forbid_proxy":*app.forbid_proxy.borrow()}))
}
async fn rewrite(app: &App, p: &Play, access: &str, base: &str, input: &str) -> Result<String> {
    static URI: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"URI="([^"]+)""#).unwrap());
    let base = net::valid_url(base)?;
    let mut out = String::new();
    let mut resources = BTreeMap::new();
    for raw in input.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            let mut rewritten = line.to_owned();
            for captures in URI.captures_iter(line) {
                let matched = captures.get(0).unwrap();
                let href = captures.get(1).unwrap().as_str();
                let target = base
                    .join(href)
                    .map_err(|_| Error::bad("HLS 资源地址无效"))?;
                let id = map_asset(
                    p,
                    target.as_str(),
                    &mut resources,
                    line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-SESSION-KEY:"),
                )?;
                rewritten =
                    rewritten.replace(matched.as_str(), &format!("URI=\"/media/{access}/{id}\""));
            }
            out.push_str(&rewritten);
        } else if !line.is_empty() {
            let target = base
                .join(line)
                .map_err(|_| Error::bad("HLS 资源地址无效"))?;
            let id = map_asset(p, target.as_str(), &mut resources, false)?;
            out.push_str(&format!("/media/{access}/{id}"));
        }
        out.push('\n');
    }
    register_assets(app, p, &resources).await?;
    Ok(out)
}
pub async fn proxy(
    app: &App,
    access: &str,
    asset_id: &str,
    method: Method,
    headers: HeaderMap,
) -> Result<Response> {
    let p: Play = sqlx::query_as("SELECT * FROM plays WHERE access_hash=?")
        .bind(hash(access))
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::unauthorized)?;
    proxy_play(app, &p, Some(access), asset_id, method, headers).await
}
pub async fn proxy_play(
    app: &App,
    p: &Play,
    access: Option<&str>,
    asset_id: &str,
    method: Method,
    headers: HeaderMap,
) -> Result<Response> {
    valid(app, p).await?;
    proxy_allowed(app, p).await?;
    if p.mode != "proxy" {
        return Err(Error::denied("请先明确选择转发重试"));
    }
    let url: String = sqlx::query_scalar("SELECT url FROM media_assets WHERE play_id=? AND id=?")
        .bind(&p.id)
        .bind(asset_id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)?;
    let mut upstream_request = headers.clone();
    if net::valid_url(&url)?
        .path()
        .to_ascii_lowercase()
        .ends_with(".m3u8")
    {
        // 重写后的清单长度与源不同，不能沿用源的字节区间。
        upstream_request.remove("range");
        upstream_request.remove("if-range");
    }
    let response = net::request(app, method.clone(), &url, &upstream_request).await?;
    valid(app, &get(app, &p.id).await?).await?;
    let status = response.status();
    let upstream_headers = response.headers().clone();
    let final_url = response.url().to_string();
    if !(status.is_success()
        || status == StatusCode::NOT_MODIFIED
        || status == StatusCode::RANGE_NOT_SATISFIABLE)
    {
        event(
            app,
            p,
            "upstream_error",
            &format!("转发上游返回 HTTP {}", status.as_u16()),
        )
        .await?;
        return Err(Error::bad(format!("媒体源返回 HTTP {}", status.as_u16())));
    }
    let mut builder = Response::builder()
        .status(status)
        .header("cache-control", "no-store")
        .header("referrer-policy", "no-referrer");
    if method == Method::HEAD {
        for k in [
            "content-type",
            "content-length",
            "content-range",
            "accept-ranges",
            "etag",
            "last-modified",
        ] {
            if let Some(v) = upstream_headers.get(k) {
                builder = builder.header(k, v);
            }
        }
        return Ok(builder.body(Body::empty()).unwrap());
    }
    let mut stream = response.bytes_stream();
    let first = match stream.next().await {
        Some(Ok(v)) => v,
        Some(Err(_)) => return Err(Error::bad("上游媒体读取失败")),
        None => bytes::Bytes::new(),
    };
    proxy_allowed(app, p).await?;
    valid(app, &get(app, &p.id).await?).await?;
    if p.state != "transferring" && p.state != "progress" {
        sqlx::query("UPDATE plays SET state='transferring' WHERE id=? AND ended=0 AND state NOT IN ('progress','ended')").bind(&p.id).execute(&app.db).await?;
        event(
            app,
            p,
            "bytes_received",
            "服务端开始收到转发资源；客户端解码状态需结合进度与实际画面判断",
        )
        .await?;
    }
    if probe::is_hls(&first) {
        let mut bytes = first.to_vec();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::bad("HLS 清单读取失败"))?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Error::bad("HLS 清单过大"));
            }
            bytes.extend(chunk);
        }
        proxy_allowed(app, p).await?;
        valid(app, &get(app, &p.id).await?).await?;
        let owned_access;
        let access = if let Some(access) = access {
            access
        } else {
            owned_access = random();
            sqlx::query("UPDATE plays SET access_hash=? WHERE id=?")
                .bind(hash(&owned_access))
                .bind(&p.id)
                .execute(&app.db)
                .await?;
            &owned_access
        };
        let source = std::str::from_utf8(&bytes).map_err(|_| Error::bad("HLS 清单编码无效"))?;
        let body = rewrite(app, p, access, &final_url, source).await?;
        valid(app, &get(app, &p.id).await?).await?;
        tracing::info!(play=%p.id, upstream_status=status.as_u16(), requested_range=headers.contains_key("range"), bytes=body.len(), "已交付完整重写 HLS 清单");
        return Ok(builder
            .status(StatusCode::OK)
            .header("content-type", "application/vnd.apple.mpegurl")
            .body(Body::from(body))
            .unwrap());
    }
    for k in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "etag",
        "last-modified",
    ] {
        if let Some(v) = upstream_headers.get(k) {
            builder = builder.header(k, v);
        }
    }
    let app = app.clone();
    let p = p.clone();
    let mut policy = app.forbid_proxy.subscribe();
    let mut access_updates = app.access_changes.subscribe();
    let body = async_stream::try_stream! {
        let blocked=*policy.borrow_and_update();
        if blocked{Err::<(),std::io::Error>(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"禁止转发已开启"))?;}
        let current=get(&app,&p.id).await.map_err(|_|std::io::Error::other("播放会话失效"))?;
        valid(&app,&current).await.map_err(|_|std::io::Error::new(std::io::ErrorKind::PermissionDenied,"播放授权已失效"))?;
        yield first;
        let mut checked=std::time::Instant::now();
        loop {
            let next=tokio::select! {
                biased;
                _=policy.changed()=>{
                    if *policy.borrow_and_update(){Some(Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"禁止转发已开启")))}else{continue}
                },
                _=access_updates.changed()=>{
                    access_updates.borrow_and_update();
                    let check = async {
                        let current=get(&app,&p.id).await.map_err(|_|std::io::Error::other("播放会话失效"))?;
                        valid(&app,&current).await.map_err(|_|std::io::Error::new(std::io::ErrorKind::PermissionDenied,"播放授权已失效"))
                    }.await;
                    if let Err(error)=check {Some(Err(error))} else {continue}
                },
                next=stream.next()=>next.map(|r|r.map_err(|_|std::io::Error::other("上游媒体读取中断"))),
            };
            if let Some(chunk)=next {
                let chunk=chunk?;
                if checked.elapsed().as_secs()>=2 {
                    let current=get(&app,&p.id).await.map_err(|_|std::io::Error::other("播放会话失效"))?;
                    valid(&app,&current).await.map_err(|_|std::io::Error::new(std::io::ErrorKind::PermissionDenied,"播放授权已失效"))?;
                    checked=std::time::Instant::now();
                }
                yield chunk;
            }else{
                break;
            }
        }
    };
    let body = body.map(|chunk: std::io::Result<bytes::Bytes>| chunk);
    Ok(builder.body(Body::from_stream(body)).unwrap())
}
