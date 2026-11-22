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
    #[serde(default)]
    kind: String,
}

impl Choice {
    fn is_hls(&self) -> bool {
        self.kind == "hls"
            || self.kind.is_empty()
                && net::valid_url(&self.media_url)
                    .is_ok_and(|url| url.path().to_ascii_lowercase().ends_with(".m3u8"))
    }
    fn play_link(&self) -> String {
        if self.is_hls() {
            format!("{}/stream.m3u8", self.link.rsplit_once('/').unwrap().0)
        } else {
            self.link.clone()
        }
    }
    fn delivery_link(&self, direct_hls: bool) -> String {
        if direct_hls {
            self.link.clone()
        } else {
            self.play_link()
        }
    }
    fn quality(&self) -> Value {
        // 此字段是原生菜单选择码；HLS 清单的带宽仍来自真实探测字段。
        json!({"resolution":self.resolution,"bitrate":self.bitrate,"progressive":true,"is_m3u8":false})
    }
}

fn choice_index(selector: &str) -> Result<usize> {
    let index = match selector.split_once('.') {
        Some((index, "m3u8" | "mp4" | "mkv" | "webm" | "ts")) => index,
        Some(_) => return Err(Error::bad("播放来源地址无效")),
        None => selector,
    };
    index.parse().map_err(|_| Error::bad("播放来源序号无效"))
}

pub(crate) fn client_link(link: &str, kind: &str, direct_hls: bool) -> Result<String> {
    if kind == "hls" && !direct_hls {
        // 原生 HLS 的 play/play 通道由客户端补上服务地址，参考 fnmedia-ext。
        // STRM 的 MP4 直链通道仍交付绝对地址。
        Ok(net::valid_url(link)?.path().to_owned())
    } else {
        Ok(link.to_owned())
    }
}

fn matches_link(expected: &str, actual: &str, relative: bool) -> bool {
    if relative {
        net::valid_url(expected).is_ok_and(|url| url.path() == actual)
    } else {
        expected == actual
    }
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
    #[sqlx(skip)]
    linked_play_id: Option<String>,
}

fn opaque_link(link: &str) -> bool {
    link.len() == 36
        && link.bytes().enumerate().all(|(index, value)| {
            if [8, 13, 18, 23].contains(&index) {
                value == b'-'
            } else {
                value.is_ascii_hexdigit()
            }
        })
}

fn record_binding(menu: &str, play: &str) -> String {
    // 原生 HLS 的媒体地址可以在换源时保持不变，进度权威仍须按实际会话区分。
    format!("fntv-menu:{menu}:play:{play}")
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
    async fn current_choice(&self, app: &App) -> Result<Choice> {
        let play = playback::get(
            app,
            self.active_play_id
                .as_deref()
                .ok_or_else(|| Error::bad("播放来源关联已失效"))?,
        )
        .await?;
        self.choices()?
            .into_iter()
            .find(|choice| choice.episode_id == play.episode_id)
            .ok_or_else(|| Error::bad("播放来源关联已失效"))
    }
}

pub(crate) fn origin(app: &App, headers: &HeaderMap) -> Result<String> {
    let origin = if app.public_url.is_empty() {
        let first = |key: &str| {
            headers
                .get(key)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let scheme = match first("x-forwarded-proto") {
            Some("https") => "https",
            Some("http") | None => "http",
            _ => return Err(Error::bad("反向代理协议无效")),
        };
        let host = first("x-forwarded-host")
            .or_else(|| first("host"))
            .ok_or_else(|| Error::bad("缺少服务地址"))?;
        let parsed = net::valid_url(&format!("{scheme}://{host}"))?;
        if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(Error::bad("反向代理服务地址无效"));
        }
        parsed.origin().ascii_serialization()
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
    if opaque_link(link) {
        let binding: Option<(String, String)> = sqlx::query_as(
            "SELECT menu_id,play_id FROM fntv_record_links WHERE auth_hash=? AND link_hash=?",
        )
        .bind(&who.auth_hash)
        .bind(hash(link))
        .fetch_optional(&app.db)
        .await?;
        let Some((menu_id, play_id)) = binding else {
            return Ok(None);
        };
        let mut menu: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE id=?")
            .bind(menu_id)
            .fetch_one(&app.db)
            .await?;
        menu.authorized(who)?;
        let play = playback::owned(app, who, &play_id).await?;
        let choice = menu
            .choices()?
            .into_iter()
            .find(|choice| choice.episode_id == play.episode_id)
            .ok_or_else(|| Error::bad("播放来源关联已失效"))?;
        menu.linked_play_id = Some(play_id);
        return Ok(Some((menu, choice)));
    }
    let relative = link.starts_with('/') && !link.starts_with("//");
    let parsed = if relative {
        url::Url::parse(&format!("http://relative.invalid{link}"))
    } else {
        url::Url::parse(link)
    };
    let Ok(url) = parsed else {
        return Ok(None);
    };
    let parts: Vec<_> = url.path().split('/').collect();
    if parts.len() == 7 && parts[..6] == ["", "v", "api", "v1", "media", "range"] {
        let menu: Option<Menu> = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND (reference_episode_id=? OR active_play_id IN (SELECT id FROM plays WHERE episode_id=?)) AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
            .bind(&who.auth_hash).bind(parts[6]).bind(parts[6]).bind(now()).fetch_optional(&app.db).await?;
        let Some(menu) = menu else { return Ok(None) };
        menu.valid(who)?;
        if !relative && url.origin() != net::valid_url(&menu.origin)?.origin() {
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
    if parts[4] == "stream.m3u8" {
        let choice = menu.current_choice(app).await?;
        if !matches_link(&choice.play_link(), link, relative) || !choice.is_hls() {
            return Err(Error::bad("播放地址与来源菜单不匹配"));
        }
        return Ok(Some((menu, choice)));
    }
    let index = choice_index(parts[4])?;
    let choice = menu
        .choices()?
        .get(index)
        .cloned()
        .ok_or_else(|| Error::bad("播放来源序号无效"))?;
    if !matches_link(&choice.link, link, relative) {
        return Err(Error::bad("播放地址与来源菜单不匹配"));
    }
    Ok(Some((menu, choice)))
}

async fn bind_native_record_link(
    app: &App,
    who: &Identity,
    link: &str,
    body: &Value,
    reference_required: bool,
) -> Result<Option<(Menu, Choice)>> {
    if !opaque_link(link) {
        return Ok(None);
    }
    let media = text(body, "media_guid");
    let item = text(body, "item_guid");
    if reference_required && media.is_empty() && item.is_empty() {
        return Ok(None);
    }
    let menus: Vec<Menu> = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND closed=0 AND expires_at>? ORDER BY rowid DESC")
        .bind(&who.auth_hash).bind(now()).fetch_all(&app.db).await?;
    let mut matching = Vec::new();
    for menu in menus {
        let choice = menu.current_choice(app).await?;
        if body["bitrate"]
            .as_u64()
            .is_some_and(|code| code >= 90_000_000 && code != choice.bitrate)
        {
            continue;
        }
        let e = catalog::episode(app, &choice.episode_id).await?;
        if fntv::validate_item_media(app, who, &item, &e)
            .await
            .is_err()
            || fntv::validate_item_media(app, who, &media, &e)
                .await
                .is_err()
        {
            continue;
        }
        validate_choice(app, who, &menu, &choice).await?;
        matching.push((menu, choice));
    }
    if matching.len() != 1 {
        return Ok(None);
    }
    let (menu, choice) = matching.remove(0);
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut fresh: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE id=?")
        .bind(&menu.id)
        .fetch_one(&mut *tx)
        .await?;
    fresh.valid(who)?;
    if fresh.active_play_id != menu.active_play_id {
        return Ok(None);
    }
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT menu_id,play_id FROM fntv_record_links WHERE auth_hash=? AND link_hash=?",
    )
    .bind(&who.auth_hash)
    .bind(hash(link))
    .fetch_optional(&mut *tx)
    .await?;
    let play_id = fresh
        .active_play_id
        .as_deref()
        .ok_or_else(|| Error::bad("播放来源关联已失效"))?;
    if existing
        .as_ref()
        .is_some_and(|(menu_id, existing_play)| menu_id != &fresh.id || existing_play != play_id)
    {
        return Ok(None);
    }
    sqlx::query("INSERT OR IGNORE INTO fntv_record_links(auth_hash,link_hash,menu_id,play_id) VALUES(?,?,?,?)")
        .bind(&who.auth_hash)
        .bind(hash(link))
        .bind(&fresh.id)
        .bind(play_id)
        .execute(&mut *tx)
        .await?;
    fresh.linked_play_id = fresh.active_play_id.clone();
    tx.commit().await?;
    Ok(Some((fresh, choice)))
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
    probe::require_available(&probe::get(app, &e).await?)?;
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
                .bind(hash(record_binding(&menu.id, id)))
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
        .bind(hash(record_binding(&menu.id, &id)))
        .bind(current.recording_disabled)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if let Some(play) = &fresh.active_play_id {
        sqlx::query("UPDATE plays SET native_link_hash=? WHERE id=? AND auth_hash=? AND ended=0")
            .bind(hash(record_binding(&fresh.id, play)))
            .bind(play)
            .bind(&who.auth_hash)
            .execute(&mut *tx)
            .await?;
    }
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
    create_menu(app, who, e, origin, current_play, false).await
}

pub(crate) async fn proxy_menu(
    app: &App,
    who: &Identity,
    e: &Episode,
    origin: &str,
    current_play: &str,
) -> Result<()> {
    create_menu(app, who, e, origin, current_play, true).await?;
    Ok(())
}

async fn create_menu(
    app: &App,
    who: &Identity,
    e: &Episode,
    origin: &str,
    current_play: &str,
    force_single: bool,
) -> Result<Option<Value>> {
    let sources = fntv::source_choices(app, who, &e.id, false).await?;
    let files = sources["files"]
        .as_array()
        .ok_or_else(|| Error::bad("没有可用的播放来源"))?;
    let current = catalog::episode(app, &e.id).await?;
    if !force_single
        && files.len() <= 1
        && probe::cached(&current).is_none_or(|info| info.kind != "hls")
    {
        return Ok(None);
    }
    let id = guid(&[&random()]);
    let token = random();
    let mut choices = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (target, _, source) =
            catalog::available(app, &who.user.id, &text(file, "guid")).await?;
        let kind = probe::cached(&target).unwrap_or_default().kind;
        let extension = if kind == "hls" { "m3u8" } else { "mp4" };
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
            link: format!("{origin}/v/native-menu/{token}/{index}.{extension}"),
            kind,
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
        .bind(hash(record_binding(&id, current_play)))
        .bind(current_play)
        .execute(&app.db)
        .await?;
    Ok(Some(menu_payload(
        &choices,
        selected,
        selected,
        *app.forbid_proxy.borrow(),
    )))
}

fn direct_order(choices: &[Choice], reference: usize, direct_hls: bool) -> Vec<usize> {
    // STRM 默认读取第一项。HLS 直链先放本次选择的来源，避免悄悄改源并串用续播位置。
    // 原生 qualities 保持画质排序，其他直链来源也沿用原顺序；顺序由初始参考集固定。
    let mut order: Vec<_> = (0..choices.len()).collect();
    if direct_hls && choices[reference].is_hls() {
        order.remove(reference);
        order.insert(0, reference);
    }
    order
}

fn menu_payload(choices: &[Choice], selected: usize, reference: usize, direct_hls: bool) -> Value {
    let order = direct_order(choices, reference, direct_hls);
    let direct = if choices[selected].is_hls() && !direct_hls {
        Vec::new()
    } else {
        order
            .iter()
            .map(|index| {
                let choice = &choices[*index];
                let mut quality = choice.quality();
                quality["url"] = json!(choice.link);
                if choice.is_hls() {
                    quality["is_m3u8"] = json!(true);
                    quality["progressive"] = json!(false);
                }
                quality
            })
            .collect()
    };
    let selected_index = if direct.is_empty() {
        selected
    } else {
        order.iter().position(|index| *index == selected).unwrap()
    };
    json!({"qualities":choices.iter().map(Choice::quality).collect::<Vec<_>>(),
        "direct_link_qualities":direct,"selected_quality_index":selected_index,
        "selected_quality":choices[selected].quality()})
}

pub(crate) async fn current_link(
    app: &App,
    who: &Identity,
    episode: &str,
) -> Result<Option<String>> {
    let menu: Option<Menu> = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND (reference_episode_id=? OR active_play_id IN (SELECT id FROM plays WHERE episode_id=?)) AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
        .bind(&who.auth_hash).bind(episode).bind(episode).bind(now()).fetch_optional(&app.db).await?;
    match menu {
        Some(menu) => Ok(Some(
            menu.current_choice(app)
                .await?
                .delivery_link(*app.forbid_proxy.borrow()),
        )),
        None => Ok(None),
    }
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
    Ok(Some(menu_payload(
        &choices,
        choices
            .iter()
            .position(|choice| choice.episode_id == selected.episode_id)
            .unwrap_or(0),
        choices
            .iter()
            .position(|choice| choice.episode_id == menu.reference_episode_id)
            .ok_or_else(|| Error::bad("播放来源菜单已失效，请重新播放"))?,
        *app.forbid_proxy.borrow(),
    )))
}

pub(crate) async fn record(app: &App, who: &Identity, body: &Value) -> Result<bool> {
    let link = text(body, "play_link");
    let parsed = url::Url::parse(&link).ok();
    tracing::debug!(
        link_length = link.len(),
        relative = link.starts_with('/'),
        absolute = parsed.is_some(),
        query = parsed.as_ref().is_some_and(|url| url.query().is_some()),
        fragment = parsed.as_ref().is_some_and(|url| url.fragment().is_some()),
        source_code = body["bitrate"].as_u64().unwrap_or(0),
        "飞牛播放回传地址类型"
    );
    let resolved = match for_link(app, who, &link).await? {
        Some(known) => Some(known),
        None => bind_native_record_link(app, who, &link, body, true).await?,
    };
    let Some((menu, choice)) = resolved else {
        return Ok(false);
    };
    if menu.recording_disabled || menu.closed {
        return Ok(true);
    }
    if menu
        .linked_play_id
        .as_ref()
        .is_some_and(|id| Some(id) != menu.active_play_id.as_ref())
    {
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
    let Some(id) = &menu.active_play_id else {
        return Ok(true);
    };
    let p = playback::owned(app, who, id).await?;
    if p.episode_id != e.id || p.ended {
        return Ok(true);
    }
    let info = probe::get(app, &e).await?;
    let position = body["ts"]
        .as_f64()
        .ok_or_else(|| Error::bad("观看位置无效"))?;
    playback::record(
        app,
        &p,
        position,
        info.duration,
        Some(&record_binding(&menu.id, id)),
    )
    .await?;
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
    let linked = query
        .get("playlink")
        .or_else(|| query.get("play_link"))
        .map(String::as_str)
        .or_else(|| {
            headers
                .get("play-link")
                .and_then(|value| value.to_str().ok())
        });
    let menu = if let Some(link) = linked {
        let resolved = match for_link(app, who, link).await? {
            Some(known) => Some(known),
            None => {
                // SDK 在首次媒体读取时已经附带独立 UUID，进度回传尚未绑定它。
                bind_native_record_link(app, who, link, &json!({"media_guid":e.id}), true).await?
            }
        };
        resolved.map(|(menu, _)| menu)
    } else {
        sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND (reference_episode_id=? OR active_play_id IN (SELECT id FROM plays WHERE episode_id=?)) AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
            .bind(&who.auth_hash).bind(&e.id).bind(&e.id).bind(now()).fetch_optional(&app.db).await?
    };
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
    fntv::validate_item_media(
        app,
        who,
        &e.id,
        &catalog::episode(app, &choice.episode_id).await?,
    )
    .await?;
    let link = choice.play_link();
    let parsed = url::Url::parse(&link).map_err(|_| Error::bad("播放来源关联已失效"))?;
    let parts: Vec<_> = parsed.path().split('/').collect();
    let mut request = Request::builder()
        .method(method)
        .body(axum::body::Body::empty())
        .unwrap();
    *request.headers_mut() = headers;
    Ok(Some(
        media_inner(app, parts[3], parts[4], request, true).await?,
    ))
}

pub(crate) async fn selected(
    app: &App,
    who: &Identity,
    e: &Episode,
    body: &Value,
) -> Result<Option<(Episode, String)>> {
    let code = body
        .get("bitrate")
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()));
    let index = body.get("direct_link_quality_index");
    if code.is_none() && index.is_none() {
        return Ok(None);
    }
    if index.is_none() && code.is_none_or(|number| number < 90_000_000) {
        return Ok(None);
    }
    let menu: Menu = sqlx::query_as("SELECT * FROM fntv_playback_menus WHERE auth_hash=? AND (reference_episode_id=? OR active_play_id IN (SELECT id FROM plays WHERE episode_id=?)) AND closed=0 AND expires_at>? ORDER BY rowid DESC LIMIT 1")
        .bind(&who.auth_hash).bind(&e.id).bind(&e.id).bind(now()).fetch_optional(&app.db).await?
        .ok_or_else(||Error::bad("播放来源菜单已失效，请重新播放"))?;
    let choices = menu.choices()?;
    let choice = if let Some(index) = index {
        let number = index
            .as_u64()
            .or_else(|| index.as_str()?.parse().ok())
            .ok_or_else(|| Error::bad("播放来源序号无效"))?;
        let reference = choices
            .iter()
            .position(|choice| choice.episode_id == menu.reference_episode_id)
            .ok_or_else(|| Error::bad("播放来源菜单已失效，请重新播放"))?;
        direct_order(&choices, reference, *app.forbid_proxy.borrow())
            .get(number as usize)
            .and_then(|index| choices.get(*index))
    } else {
        let number = code.ok_or_else(|| Error::bad("播放来源选择无效"))?;
        choices.iter().find(|choice| choice.bitrate == number)
    }
    .ok_or_else(|| Error::bad("播放来源选择已失效，请刷新来源"))?;
    let target = validate_choice(app, who, &menu, choice).await?;
    activate(app, who, &menu, choice).await?;
    Ok(Some((
        target,
        choice.delivery_link(*app.forbid_proxy.borrow()),
    )))
}

pub(crate) async fn quit(app: &App, who: &Identity, link: &str) -> Result<bool> {
    let Some((menu, choice)) = for_link(app, who, link).await? else {
        return Ok(false);
    };
    if menu.closed {
        return Ok(true);
    }
    if menu
        .linked_play_id
        .as_ref()
        .is_some_and(|id| Some(id) != menu.active_play_id.as_ref())
    {
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
    let link = text(body, "playLink");
    let resolved = match for_link(app, who, &link).await? {
        Some(known) => Some(known),
        None => bind_native_record_link(app, who, &link, body, false).await?,
    };
    let Some((menu, choice)) = resolved else {
        return Ok(None);
    };
    if menu
        .linked_play_id
        .as_ref()
        .is_some_and(|id| Some(id) != menu.active_play_id.as_ref())
    {
        return Err(Error::bad("播放会话已结束，请重新播放"));
    }
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
            if choice.is_hls() && !target.is_hls() {
                return Err(Error::bad("来源媒体格式不同，请重新进入该来源播放"));
            }
            let play = activate(app, who, &menu, &target).await?;
            if opaque_link(&link) {
                // 原位 RPC 换源继续使用相同 SDK 身份；play/play 换源则保留旧身份，
                // 防止客户端随后发送的旧 quit 结束新播放。
                sqlx::query("UPDATE fntv_record_links SET play_id=? WHERE auth_hash=? AND link_hash=? AND menu_id=?")
                    .bind(&play.id).bind(&who.auth_hash).bind(hash(&link)).bind(&menu.id)
                    .execute(&app.db).await?;
            }
            return Ok(Some(
                json!({"reqid":body["reqid"],"result":"succ","play_link":client_link(&target.delivery_link(*app.forbid_proxy.borrow()),if target.is_hls(){"hls"}else{"video"},*app.forbid_proxy.borrow())?}),
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
    Path((token, selector)): Path<(String, String)>,
    request: Request,
) -> Response {
    media_inner(&app, &token, &selector, request, false)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn media_inner(
    app: &App,
    token: &str,
    selector: &str,
    request: Request,
    force_proxy: bool,
) -> Result<Response> {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(Error::bad("媒体仅支持 GET/HEAD"));
    }
    let menu = get(app, token).await?;
    let who = identity(app, &menu).await?;
    let choice = if selector == "stream.m3u8" {
        let choice = menu.current_choice(app).await?;
        if !choice.is_hls() {
            return Err(Error::bad("播放来源格式已变更，请重新播放"));
        }
        choice
    } else {
        menu.choices()?
            .get(choice_index(selector)?)
            .cloned()
            .ok_or_else(|| Error::bad("播放来源序号无效"))?
    };
    let e = validate_choice(app, &who, &menu, &choice).await?;
    let carries_auth = ["authorization", "cookie", "authx"]
        .iter()
        .any(|name| request.headers().contains_key(*name));
    if request.method() == Method::HEAD {
        let info = probe::get(app, &e).await?;
        probe::require_available(&info)?;
        let play = playback::owned(
            app,
            &who,
            menu.active_play_id
                .as_deref()
                .ok_or_else(|| Error::bad("播放来源关联已失效"))?,
        )
        .await?;
        if play.mode == "direct" && !carries_auth && !force_proxy && info.kind != "hls" {
            return Ok(Redirect::temporary(&e.url).into_response());
        }
        if play.mode == "proxy" || force_proxy || carries_auth {
            playback::proxy_allowed(app, &play).await?;
        }
        let mut builder = Response::builder()
            .header("cache-control", "no-store")
            .header(
                "content-type",
                if info.kind == "hls" {
                    "application/vnd.apple.mpegurl"
                } else {
                    "video/mp4"
                },
            );
        if info.size > 0 {
            builder = builder
                .header("content-length", info.size)
                .header("accept-ranges", "bytes");
        }
        return Ok(builder.body(axum::body::Body::empty()).unwrap());
    }
    let mut p = activate(app, &who, &menu, &choice).await?;
    // 认证头只留在本站。原生 STRM 空头请求可直接跟随到源，其他端安全地同源转发。
    if p.mode == "direct" && !carries_auth && !force_proxy {
        if choice.is_hls() {
            let info = probe::get(app, &e).await?;
            let quality = probe::quality(&info);
            let bandwidth = quality["bitrate"].as_f64().unwrap_or(0.0) as u64;
            let width = quality["width"].as_f64().unwrap_or(0.0) as u64;
            let height = quality["height"].as_f64().unwrap_or(0.0) as u64;
            let resolution = if width > 0 && height > 0 {
                format!(",RESOLUTION={width}x{height}")
            } else {
                String::new()
            };
            let source = net::valid_url(&e.url)?;
            let manifest = format!(
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH={}{resolution}\n{source}\n",
                bandwidth.max(1)
            );
            return Ok(Response::builder()
                .header("cache-control", "no-store")
                .header("content-type", "application/vnd.apple.mpegurl")
                .body(axum::body::Body::from(manifest))
                .unwrap());
        }
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
