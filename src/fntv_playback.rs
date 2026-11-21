//! 飞牛播放中的清晰度菜单承载来源选择，菜单快照与实际播放会话分开。
use crate::{access, auth, catalog, core::*, fntv, net, playback, probe};
use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, Method},
    response::{IntoResponse, Redirect, Response},
};
use catalog::Episode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize)]
struct Choice {
    episode_id: String,
    source_id: String,
    bitrate: u64,
    resolution: String,
    media_url: String,
    link: String,
}

#[derive(sqlx::FromRow)]
struct Menu {
    id: String,
    user_id: String,
    auth_hash: String,
    reference_episode_id: String,
    choices: String,
    origin: String,
    active_play_id: Option<String>,
    expires_at: i64,
    recording_disabled: bool,
    closed: bool,
}

impl Menu {
    fn choices(&self) -> Result<Vec<Choice>> {
        serde_json::from_str(&self.choices)
            .map_err(|_| Error::bad("播放来源菜单已失效，请重新播放"))
    }
    fn authorized(&self, who: &Identity) -> Result<()> {
        if self.user_id != who.user.id || self.auth_hash != who.auth_hash {
            return Err(Error::denied("播放来源菜单不属于当前登录会话"));
        }
        if self.expires_at <= now() {
            return Err(Error::bad("播放来源菜单已结束，请重新播放"));
        }
        Ok(())
    }
    fn valid(&self, who: &Identity) -> Result<()> {
        self.authorized(who)?;
        if self.closed {
            return Err(Error::bad("播放来源菜单已结束，请重新播放"));
        }
        Ok(())
    }
}

pub(crate) fn origin(app: &App, headers: &HeaderMap) -> Result<String> {
    let origin = if app.public_url.is_empty() {
        format!(
            "http://{}",
            headers
                .get("host")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::bad("缺少服务地址"))?
        )
    } else {
        app.public_url.clone()
    };
    let parsed = net::valid_url(&origin)?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(Error::bad("服务公开地址不能包含查询参数或片段"));
    }
    Ok(origin.trim_end_matches('/').to_owned())
}

async fn identity(app: &App, menu: &Menu) -> Result<Identity> {
    let who = auth::identity_for_hash(app, &menu.auth_hash).await?;
    menu.valid(&who)?;
    Ok(who)
}

async fn get(app: &App, access: &str) -> Result<Menu> {
    sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE access_hash=?")
        .bind(hash(access))
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::unauthorized)
}

async fn for_link(app: &App, who: &Identity, link: &str) -> Result<Option<(Menu, Choice)>> {
    let Ok(url) = url::Url::parse(link) else {
        return Ok(None);
    };
    let parts: Vec<_> = url.path().split('/').collect();
    if parts.len() == 7 && parts[..6] == ["", "v", "api", "v1", "media", "range"] {
        let menu: Option<Menu> = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND reference_episode_id=? AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
            .bind(&who.auth_hash).bind(parts[6]).bind(now()).fetch_optional(&app.db).await?;
        let Some(menu) = menu else { return Ok(None) };
        menu.valid(who)?;
        if url.origin() != net::valid_url(&menu.origin)?.origin() {
            return Err(Error::bad("播放地址不属于本站"));
        }
        let Some(id) = &menu.active_play_id else {
            return Ok(None);
        };
        let p = playback::owned(app, who, id).await?;
        let choice = menu
            .choices()?
            .into_iter()
            .find(|choice| choice.episode_id == p.episode_id)
            .ok_or_else(|| Error::bad("播放来源关联已失效"))?;
        return Ok(Some((menu, choice)));
    }
    if parts.len() != 5 || parts[..3] != ["", "v", "native-menu"] {
        return Ok(None);
    }
    let menu = get(app, parts[3]).await?;
    menu.authorized(who)?;
    let index = parts[4]
        .parse::<usize>()
        .map_err(|_| Error::bad("播放来源序号无效"))?;
    let choice = menu
        .choices()?
        .get(index)
        .cloned()
        .ok_or_else(|| Error::bad("播放来源序号无效"))?;
    if choice.link != link {
        return Err(Error::bad("播放地址与来源菜单不匹配"));
    }
    Ok(Some((menu, choice)))
}

async fn validate_choice(
    app: &App,
    who: &Identity,
    menu: &Menu,
    choice: &Choice,
) -> Result<Episode> {
    menu.valid(who)?;
    let (e, f, _) = catalog::available(app, &who.user.id, &choice.episode_id).await?;
    if e.url != choice.media_url {
        return Err(Error::bad("播放地址已更新，请刷新来源"));
    }
    let original = catalog::episode(app, &menu.reference_episode_id).await?;
    if original.id != e.id {
        let hint = catalog::switch_episode_hint(
            app,
            &who.user.id,
            &f.id,
            &original.film_id,
            &original.id,
            Some(&e.line_name),
            0.0,
        )
        .await?;
        if hint["matched"] != true || text(&hint, "episode_id") != e.id {
            return Err(Error::bad("播放源已更新或无法确认同集，请重新选择来源"));
        }
    }
    Ok(e)
}

async fn activate(
    app: &App,
    who: &Identity,
    menu: &Menu,
    choice: &Choice,
) -> Result<playback::Play> {
    let e = validate_choice(app, who, menu, choice).await?;
    // 探测不占用切换锁；切换事务只处理已经可播放的媒体。
    probe::get(app, &e).await?;
    let _lock = app.native_switch_lock.lock().await;
    let fresh: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE id=?")
        .bind(&menu.id)
        .fetch_one(&app.db)
        .await?;
    fresh.valid(who)?;
    if let Some(id) = &fresh.active_play_id {
        let p = playback::owned(app, who, id).await?;
        if p.episode_id == e.id && !p.ended {
            sqlx::query("UPDATE plays SET native_link_hash=? WHERE id=?")
                .bind(hash(&choice.link))
                .bind(id)
                .execute(&app.db)
                .await?;
            return playback::get(app, id).await;
        }
    }
    let started = playback::start(app, who, &e.id).await?;
    let id = text(&started, "id");
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let current: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE id=?")
        .bind(&menu.id)
        .fetch_one(&mut *tx)
        .await?;
    current.valid(who)?;
    if !access::source_allowed_on(&mut tx, &who.user.id, &choice.source_id).await? {
        return Err(Error::denied(access::UNAUTHORIZED_SOURCE));
    }
    sqlx::query("UPDATE plays SET native_link_hash=?,recording_disabled=? WHERE id=?")
        .bind(hash(&choice.link))
        .bind(current.recording_disabled)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE fntv_playback_menus SET active_play_id=? WHERE id=?")
        .bind(&id)
        .bind(&menu.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    fntv::finish_source_switch(app, who, &catalog::film(app, &e.film_id).await?, &id).await?;
    playback::get(app, &id).await
}

pub(crate) async fn stream_menu(
    app: &App,
    who: &Identity,
    e: &Episode,
    origin: &str,
    current_play: &str,
) -> Result<Option<Value>> {
    let sources = fntv::source_choices(app, who, &e.id, false).await?;
    let files = sources["files"]
        .as_array()
        .ok_or_else(|| Error::bad("没有可用的播放来源"))?;
    if files.len() <= 1 {
        return Ok(None);
    }
    let id = guid(&[&random()]);
    let token = random();
    let mut choices = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (target, _, source) =
            catalog::available(app, &who.user.id, &text(file, "guid")).await?;
        choices.push(Choice {
            episode_id: target.id.clone(),
            source_id: source.id,
            bitrate: 90_000_000 + index as u64 * 10001,
            resolution: fntv::menu_label(
                file,
                files
                    .iter()
                    .filter(|other| other["source_id"] == file["source_id"])
                    .count()
                    > 1,
            ),
            media_url: target.url,
            link: format!("{origin}/v/native-menu/{token}/{index}"),
        });
    }
    let grouped = catalog::history_film_ids(app, &who.user.id, &e.film_id).await?;
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut close = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "UPDATE fntv_playback_menus SET closed=1 WHERE auth_hash=",
    );
    close
        .push_bind(&who.auth_hash)
        .push(" AND reference_episode_id IN (SELECT id FROM episodes WHERE film_id IN (");
    let mut ids = close.separated(",");
    for id in &grouped {
        ids.push_bind(id);
    }
    close.push("))");
    close.build().execute(&mut *tx).await?;
    sqlx::query("INSERT INTO fntv_playback_menus(id,access_hash,user_id,auth_hash,reference_episode_id,choices,origin,active_play_id,expires_at,recording_disabled) SELECT ?,?,?,?,?,?,?,?,?,recording_disabled FROM plays WHERE id=?")
        .bind(&id).bind(hash(&token)).bind(&who.user.id).bind(&who.auth_hash).bind(&e.id)
        .bind(serde_json::to_string(&choices).unwrap()).bind(origin).bind(current_play).bind(now()+6*3600).bind(current_play).execute(&mut *tx).await?;
    tx.commit().await?;
    let selected = choices
        .iter()
        .position(|choice| choice.episode_id == e.id)
        .ok_or_else(|| Error::bad("当前播放源已不可用"))?;
    let allowed: std::collections::HashSet<_> = access::sources(app, &who.user.id)
        .await?
        .into_iter()
        .filter(|s| s.active())
        .map(|s| s.id)
        .collect();
    if choices
        .iter()
        .any(|choice| !allowed.contains(&choice.source_id))
    {
        return Err(Error::denied("播放来源使用范围已更新，请重新播放"));
    }
    sqlx::query("UPDATE plays SET native_link_hash=? WHERE id=?")
        .bind(hash(&choices[selected].link))
        .bind(current_play)
        .execute(&app.db)
        .await?;
    Ok(Some(
        json!({"qualities":choices.iter().map(|choice|json!({"resolution":choice.resolution,"bitrate":choice.bitrate,"progressive":true,"is_m3u8":false})).collect::<Vec<_>>(),
        "direct_link_qualities":choices.iter().map(|choice|json!({"resolution":choice.resolution,"bitrate":choice.bitrate,"progressive":true,"is_m3u8":false,"url":choice.link})).collect::<Vec<_>>(),
        "selected_quality_index":selected}),
    ))
}

pub(crate) async fn current_link(
    app: &App,
    who: &Identity,
    episode: &str,
) -> Result<Option<String>> {
    let menu: Option<Menu> = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND reference_episode_id=? AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
        .bind(&who.auth_hash).bind(episode).bind(now()).fetch_optional(&app.db).await?;
    Ok(menu
        .map(|menu| {
            menu.choices().map(|choices| {
                choices
                    .into_iter()
                    .find(|choice| choice.episode_id == episode)
                    .map(|choice| choice.link)
            })
        })
        .transpose()?
        .flatten())
}

pub(crate) async fn payload_for_link(
    app: &App,
    who: &Identity,
    link: &str,
) -> Result<Option<Value>> {
    let Some((menu, selected)) = for_link(app, who, link).await? else {
        return Ok(None);
    };
    let choices = menu.choices()?;
    for choice in &choices {
        validate_choice(app, who, &menu, choice).await?;
    }
    Ok(Some(
        json!({"qualities":choices.iter().map(|choice|json!({"resolution":choice.resolution,"bitrate":choice.bitrate,"progressive":true,"is_m3u8":false})).collect::<Vec<_>>(),
        "direct_link_qualities":choices.iter().map(|choice|json!({"resolution":choice.resolution,"bitrate":choice.bitrate,"progressive":true,"is_m3u8":false,"url":choice.link})).collect::<Vec<_>>(),
        "selected_quality_index":choices.iter().position(|choice|choice.episode_id==selected.episode_id).unwrap_or(0)}),
    ))
}

pub(crate) async fn record(app: &App, who: &Identity, body: &Value) -> Result<bool> {
    let link = text(body, "play_link");
    let Some((menu, choice)) = for_link(app, who, &link).await? else {
        return Ok(false);
    };
    if menu.recording_disabled || menu.closed {
        return Ok(true);
    }
    if body["bitrate"]
        .as_u64()
        .is_some_and(|code| code >= 90_000_000 && code != choice.bitrate)
    {
        return Ok(true);
    }
    let e = validate_choice(app, who, &menu, &choice).await?;
    fntv::validate_item_media(app, who, &text(body, "item_guid"), &e).await?;
    fntv::validate_item_media(app, who, &text(body, "media_guid"), &e).await?;
    let Some(id) = menu.active_play_id else {
        return Ok(true);
    };
    let p = playback::owned(app, who, &id).await?;
    if p.episode_id != e.id || p.ended {
        return Ok(true);
    }
    let info = probe::get(app, &e).await?;
    let position = body["ts"]
        .as_f64()
        .ok_or_else(|| Error::bad("观看位置无效"))?;
    playback::record(app, &p, position, info.duration, Some(&choice.link)).await?;
    Ok(true)
}

pub(crate) async fn range(
    app: &App,
    who: &Identity,
    e: &Episode,
    query: &std::collections::HashMap<String, String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Option<Response>> {
    let menu: Option<Menu>=sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND reference_episode_id=? AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
        .bind(&who.auth_hash).bind(&e.id).bind(now()).fetch_optional(&app.db).await?;
    let Some(menu) = menu else { return Ok(None) };
    let body = json!(query);
    let selected = selected(app, who, e, &body).await?;
    let choice = if let Some((target, _)) = selected {
        menu.choices()?
            .into_iter()
            .find(|choice| choice.episode_id == target.id)
    } else if let Some(id) = &menu.active_play_id {
        let p = playback::owned(app, who, id).await?;
        menu.choices()?
            .into_iter()
            .find(|choice| choice.episode_id == p.episode_id)
    } else {
        None
    }
    .ok_or_else(|| Error::bad("播放来源关联已失效"))?;
    let parsed = url::Url::parse(&choice.link).map_err(|_| Error::bad("播放来源关联已失效"))?;
    let parts: Vec<_> = parsed.path().split('/').collect();
    let index = parts[4]
        .parse()
        .map_err(|_| Error::bad("播放来源序号无效"))?;
    let mut request = Request::builder()
        .method(method)
        .body(axum::body::Body::empty())
        .unwrap();
    *request.headers_mut() = headers;
    Ok(Some(media_inner(app, parts[3], index, request).await?))
}

pub(crate) async fn selected(
    app: &App,
    who: &Identity,
    e: &Episode,
    body: &Value,
) -> Result<Option<(Episode, String)>> {
    let code = body.get("bitrate");
    let index = body.get("direct_link_quality_index");
    if code.is_none() && index.is_none() {
        return Ok(None);
    }
    if index.is_none()
        && code
            .and_then(Value::as_u64)
            .is_some_and(|number| number < 90_000_000)
    {
        return Ok(None);
    }
    let menu: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND reference_episode_id=? AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
        .bind(&who.auth_hash).bind(&e.id).bind(now()).fetch_optional(&app.db).await?
        .ok_or_else(||Error::bad("播放来源菜单已失效，请重新播放"))?;
    let choices = menu.choices()?;
    let choice = if let Some(index) = index {
        let number = index
            .as_u64()
            .or_else(|| index.as_str()?.parse().ok())
            .ok_or_else(|| Error::bad("播放来源序号无效"))?;
        choices.get(number as usize)
    } else {
        let number = code
            .and_then(|code| code.as_u64().or_else(|| code.as_str()?.parse().ok()))
            .ok_or_else(|| Error::bad("播放来源选择无效"))?;
        choices.iter().find(|choice| choice.bitrate == number)
    }
    .ok_or_else(|| Error::bad("播放来源选择已失效，请刷新来源"))?;
    let target = validate_choice(app, who, &menu, choice).await?;
    activate(app, who, &menu, choice).await?;
    Ok(Some((target, choice.link.clone())))
}

pub(crate) async fn quit(app: &App, who: &Identity, link: &str) -> Result<bool> {
    let Some((menu, choice)) = for_link(app, who, link).await? else {
        return Ok(false);
    };
    if menu.closed {
        return Ok(true);
    }
    if let Some(play) = menu.active_play_id {
        let p = playback::owned(app, who, &play).await?;
        if p.episode_id == choice.episode_id {
            playback::end(app, &p).await?;
        }
    }
    Ok(true)
}

pub(crate) async fn rpc(app: &App, who: &Identity, body: &Value) -> Result<Option<Value>> {
    let Some((menu, choice)) = for_link(app, who, &text(body, "playLink")).await? else {
        return Ok(None);
    };
    match text(body, "req").as_str() {
        "media.resetQuality" => {
            let code = body["quality"]["bitrate"]
                .as_u64()
                .ok_or_else(|| Error::bad("播放来源选择无效"))?;
            let target = menu
                .choices()?
                .into_iter()
                .find(|choice| choice.bitrate == code)
                .ok_or_else(|| Error::bad("播放来源选择已失效"))?;
            activate(app, who, &menu, &target).await?;
            return Ok(Some(
                json!({"reqid":body["reqid"],"result":"succ","play_link":target.link}),
            ));
        }
        "media.quit" => {
            quit(app, who, &choice.link).await?;
        }
        _ => {}
    }
    Ok(Some(json!({"reqid":body["reqid"],"result":"succ"})))
}

pub(crate) async fn media(
    State(app): State<App>,
    Path((token, index)): Path<(String, usize)>,
    request: Request,
) -> Response {
    media_inner(&app, &token, index, request)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn media_inner(app: &App, token: &str, index: usize, request: Request) -> Result<Response> {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(Error::bad("媒体仅支持 GET/HEAD"));
    }
    let menu = get(app, token).await?;
    let who = identity(app, &menu).await?;
    let choice = menu
        .choices()?
        .get(index)
        .cloned()
        .ok_or_else(|| Error::bad("播放来源序号无效"))?;
    let e = validate_choice(app, &who, &menu, &choice).await?;
    let carries_auth = ["authorization", "cookie", "authx"]
        .iter()
        .any(|name| request.headers().contains_key(*name));
    if request.method() == Method::HEAD {
        if !carries_auth {
            return Ok(Redirect::temporary(&e.url).into_response());
        }
        return Ok(Response::builder()
            .header("cache-control", "no-store")
            .body(axum::body::Body::empty())
            .unwrap());
    }
    let mut p = activate(app, &who, &menu, &choice).await?;
    // 认证头只留在本站。原生 STRM 空头请求可直接跟随到源，其他端安全地同源转发。
    if p.mode == "direct" && !carries_auth {
        return Ok(Redirect::temporary(&e.url).into_response());
    }
    if p.mode == "direct" {
        playback::fallback(app, &who, &p.id, "headers").await?;
        p = playback::get(app, &p.id).await?;
    }
    let access = hash(format!("native-proxy:{token}:{}", p.id));
    sqlx::query("UPDATE plays SET access_hash=? WHERE id=?")
        .bind(hash(&access))
        .bind(&p.id)
        .execute(&app.db)
        .await?;
    let root = playback::asset(app, &p, &e.url).await?;
    playback::proxy(
        app,
        &access,
        &root,
        request.method().clone(),
        request.headers().clone(),
    )
    .await
}
