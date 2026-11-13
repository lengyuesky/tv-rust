use crate::{core::*, net};
use axum::http::{HeaderMap, Method};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Source {
    pub id: String,
    pub subscription_id: String,
    pub source_key: String,
    pub name: String,
    pub api: String,
    pub detail: String,
    pub enabled: bool,
    pub present: bool,
    pub upstream_enabled: bool,
    pub note: String,
}
impl Source {
    pub fn active(&self) -> bool {
        self.enabled && self.present && self.upstream_enabled
    }
}
#[derive(Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Film {
    pub id: String,
    pub source_id: String,
    pub vod_id: String,
    pub title: String,
    pub year: String,
    pub kind: String,
    pub category: String,
    pub poster: String,
    pub overview: String,
    pub remarks: String,
    pub actor: String,
    pub director: String,
    pub updated_at: i64,
    pub detailed_at: i64,
}
#[derive(Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Episode {
    pub id: String,
    pub film_id: String,
    pub line_name: String,
    pub line_order: i64,
    pub title: String,
    pub number: i64,
    pub url: String,
    pub active: bool,
    pub probe: String,
    pub probed_at: i64,
}

fn version_markers(f: &Film) -> Vec<&'static str> {
    let description = format!("{} {} {}", f.title, f.remarks, f.category);
    [
        "合集",
        "合辑",
        "删减",
        "加长",
        "导演剪辑",
        "特别篇",
        "特别版",
        "重制",
        "修复",
        "剧场版",
        "抢先",
    ]
    .into_iter()
    .filter(|marker| description.contains(marker))
    .collect()
}

pub fn group_key(f: &Film) -> String {
    // 年份未知的条目无法确认是同一作品，明显不同的版本也各自展示。
    if f.year.trim().is_empty() {
        return f.id.clone();
    }
    json!([
        f.title.trim().to_lowercase(),
        f.year.trim(),
        f.kind,
        version_markers(f)
    ])
    .to_string()
}

fn episode_number(label: &str) -> Option<u32> {
    static LABEL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:第\s*([0-9零〇一二两三四五六七八九十百千]+)\s*[集话期]|(?:ep?|episode)\s*([0-9]+)|([0-9]+))$").unwrap()
    });
    let captures = LABEL.captures(label.trim())?;
    let number = captures.iter().skip(1).flatten().next()?.as_str();
    if let Ok(n) = number.parse::<u32>() {
        return (n > 0 && n <= 9999).then_some(n);
    }
    let mut total = 0;
    let mut digit = 0;
    let mut last_unit = 10000;
    for c in number.chars() {
        if let Some(n) = "零一二三四五六七八九".chars().position(|v| v == c) {
            digit = digit * 10 + n as u32;
        } else if c == '〇' {
            digit *= 10;
        } else if c == '两' {
            digit = 2;
        } else {
            let unit = match c {
                '十' => 10,
                '百' => 100,
                '千' => 1000,
                _ => return None,
            };
            if unit >= last_unit || digit > 9 {
                return None;
            }
            total += digit.max(1) * unit;
            digit = 0;
            last_unit = unit;
        }
    }
    let n = total + digit;
    (n > 0 && n <= 9999).then_some(n)
}

pub async fn switch_hint(app: &App, user: &str, target_id: &str, from_id: &str) -> Result<Value> {
    if target_id == from_id || from_id.is_empty() {
        return Ok(Value::Null);
    }
    // 只读取当前账号的原来源进度；匹配只产生选集建议，不合并收藏或进度。
    let prior: Option<(String, f64)> =
        sqlx::query_as("SELECT episode_id,position FROM progress WHERE user_id=? AND film_id=?")
            .bind(user)
            .bind(from_id)
            .fetch_optional(&app.db)
            .await?;
    let Some((episode_id, position)) = prior else {
        return Ok(
            json!({"matched":false,"message":"原来源尚无可用观看进度，请在新来源选择要播放的单集。"}),
        );
    };
    let from = film(app, from_id).await?;
    let target = film(app, target_id).await?;
    let original = episode(app, &episode_id).await?;
    let source = source(app, &from.source_id).await?;
    let mut hint = json!({"matched":false,"from_episode":original.title,"from_position":position,"from_source":source.name,
        "message":"无法确认对应单集，请核对版本并重新选集。"});
    if from.year.trim().is_empty()
        || group_key(&from) != group_key(&target)
        || !version_markers(&from).is_empty()
        || !version_markers(&target).is_empty()
    {
        hint["message"] = json!("标题、年份或版本无法确认一致，请在新来源重新选集。");
        return Ok(hint);
    }
    let candidates = episodes(app, target_id).await?;
    let matching = if from.kind == "Movie" {
        // 花絮、预告和多个剪辑不猜测为正片。
        if original.title.trim() != "正片" {
            None
        } else {
            candidates.iter().find(|e| e.title.trim() == "正片")
        }
    } else if let Some(number) = episode_number(&original.title) {
        let mut lines = BTreeMap::<i64, Vec<&Episode>>::new();
        for e in &candidates {
            if episode_number(&e.title) == Some(number) {
                lines.entry(e.line_order).or_default().push(e);
            }
        }
        // 同一线路重复标记相同集号时不猜测；线路之间的同集号可供用户切换。
        lines
            .into_values()
            .next()
            .and_then(|matches| (matches.len() == 1).then(|| matches[0]))
    } else {
        None
    };
    if let Some(e) = matching {
        hint["matched"] = json!(true);
        hint["episode_id"] = json!(e.id);
        hint["episode_title"] = json!(e.title);
        hint["message"] = json!(format!(
            "已找到对应的{}，播放前请核对版本。新来源使用自己的观看进度。",
            e.title
        ));
    }
    Ok(hint)
}
#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    cache_time: Option<i64>,
    api_site: BTreeMap<String, Value>,
}

pub fn decode(input: &str) -> Result<Value> {
    if input.len() > 4 * 1024 * 1024 {
        return Err(Error::bad("配置超过 4 MiB"));
    }
    let s = input.trim().trim_start_matches('\u{feff}');
    let data = if s.starts_with('{') {
        s.as_bytes().to_vec()
    } else {
        bs58::decode(s)
            .into_vec()
            .map_err(|_| Error::bad("配置不是有效 JSON 或 Base58"))?
    };
    let v: Value = serde_json::from_slice(&data).map_err(|_| Error::bad("配置不是有效 JSON"))?;
    let config: Config = serde_json::from_value(v.clone())
        .map_err(|_| Error::bad("配置需要 api_site 对象，cache_time 应为秒数"))?;
    if config.api_site.len() > 1000 {
        return Err(Error::bad("一份订阅最多支持 1000 个源"));
    }
    for (key, source) in &config.api_site {
        if key.is_empty() || key.len() > 512 || text(source, "name").is_empty() {
            return Err(Error::bad("源 ID 或名称无效"));
        }
        net::valid_url(&text(source, "api"))?;
        let detail = text(source, "detail");
        if !detail.is_empty() {
            net::valid_url(&detail)?;
        }
    }
    Ok(v)
}
async fn download_config(app: &App, url: &str) -> Result<String> {
    let r = net::request(app, Method::GET, url, &HeaderMap::new()).await?;
    if !r.status().is_success() {
        return Err(Error::bad(format!("订阅返回 HTTP {}", r.status().as_u16())));
    }
    String::from_utf8(net::bytes(r, 4 * 1024 * 1024, false).await?)
        .map_err(|_| Error::bad("订阅不是 UTF-8 文本"))
}
async fn apply_config(app: &App, id: &str, name: &str, url: &str, config: &Value) -> Result<()> {
    let parsed: Config =
        serde_json::from_value(config.clone()).map_err(|_| Error::bad("配置无效"))?;
    let ttl = parsed
        .cache_time
        .filter(|n| *n > 0)
        .unwrap_or(7200)
        .clamp(300, 7 * 86400);
    let mut tx = app.db.begin().await?;
    sqlx::query("INSERT INTO subscriptions(id,name,url,config,interval_seconds,updated_at,checked_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,url=excluded.url,config=excluded.config,interval_seconds=excluded.interval_seconds,updated_at=excluded.updated_at,checked_at=excluded.checked_at,error=''")
        .bind(id).bind(name).bind(url).bind(config.to_string()).bind(ttl).bind(now()).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE sources SET present=0 WHERE subscription_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for (key, v) in parsed.api_site {
        sqlx::query("INSERT INTO sources(id,subscription_id,source_key,name,api,detail,upstream_enabled,note) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(subscription_id,source_key) DO UPDATE SET name=excluded.name,api=excluded.api,detail=excluded.detail,present=1,upstream_enabled=excluded.upstream_enabled,note=excluded.note")
            .bind(guid(&["source",id,&key])).bind(id).bind(key).bind(text(&v,"name")).bind(text(&v,"api"))
            .bind(text(&v,"detail")).bind(!v["disabled"].as_bool().unwrap_or(false)).bind(text(&v,"_comment")).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM cache WHERE key LIKE 'search:%'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn import(app: &App, v: &Value) -> Result<String> {
    let _guard = app.refresh_lock.lock().await;
    let name = text(v, "name").trim().to_owned();
    let url = text(v, "url").trim().to_owned();
    if name.is_empty() || name.len() > 128 {
        return Err(Error::bad("请填写订阅名称（最多 128 字节）"));
    }
    let content = if url.is_empty() {
        text(v, "content")
    } else {
        net::valid_url(&url)?;
        download_config(app, &url).await?
    };
    let config = decode(&content)?;
    let provided = text(v, "id");
    let id = if provided.is_empty() {
        guid(&[&random()])
    } else {
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE id=?")
            .bind(&provided)
            .fetch_one(&app.db)
            .await?;
        if exists == 0 {
            return Err(Error::missing());
        }
        provided
    };
    apply_config(app, &id, &name, &url, &config).await?;
    Ok(id)
}
pub async fn refresh(app: &App, id: &str) -> Result<()> {
    let _guard = app.refresh_lock.lock().await;
    let (name, url): (String, String) =
        sqlx::query_as("SELECT name,url FROM subscriptions WHERE id=?")
            .bind(id)
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(Error::missing)?;
    if url.is_empty() {
        return Err(Error::bad("文本导入没有远端地址，请重新导入以更新"));
    }
    let result = async {
        let content = download_config(app, &url).await?;
        let config = decode(&content)?;
        apply_config(app, id, &name, &url, &config).await
    }
    .await;
    if let Err(ref e) = result {
        sqlx::query("UPDATE subscriptions SET checked_at=?,error=? WHERE id=?")
            .bind(now())
            .bind(&e.1)
            .bind(id)
            .execute(&app.db)
            .await?;
    }
    result
}
pub async fn sources(app: &App) -> Result<Vec<Source>> {
    Ok(sqlx::query_as("SELECT * FROM sources ORDER BY name,id")
        .fetch_all(&app.db)
        .await?)
}
pub async fn source(app: &App, id: &str) -> Result<Source> {
    sqlx::query_as("SELECT * FROM sources WHERE id=?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)
}
pub async fn film(app: &App, id: &str) -> Result<Film> {
    sqlx::query_as("SELECT * FROM films WHERE id=?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)
}
pub async fn episode(app: &App, id: &str) -> Result<Episode> {
    sqlx::query_as("SELECT * FROM episodes WHERE id=?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)
}
pub async fn episodes(app: &App, id: &str) -> Result<Vec<Episode>> {
    Ok(sqlx::query_as(
        "SELECT * FROM episodes WHERE film_id=? AND active=1 ORDER BY line_order,number,id",
    )
    .bind(id)
    .fetch_all(&app.db)
    .await?)
}
pub async fn available(app: &App, episode_id: &str) -> Result<(Episode, Film, Source)> {
    let e = episode(app, episode_id).await?;
    let f = film(app, &e.film_id).await?;
    let s = source(app, &f.source_id).await?;
    if !e.active || !s.active() {
        return Err(Error::bad("来源已失效或被禁用，请切换视频源"));
    }
    Ok((e, f, s))
}
pub fn api_url(base: &str, params: &[(&str, &str)]) -> Result<String> {
    let mut u = net::valid_url(base)?;
    // 配置可在 ?url= 中封装上游，参数必须附加到内嵌地址。
    let pairs = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    if let Some((_, inner)) = pairs
        .iter()
        .find(|(k, v)| k == "url" && (v.starts_with("http://") || v.starts_with("https://")))
    {
        let nested = api_url(inner, params)?;
        u.set_query(None);
        for (k, v) in pairs {
            u.query_pairs_mut()
                .append_pair(&k, if k == "url" { &nested } else { &v });
        }
    } else {
        let names = params.iter().map(|p| p.0).collect::<Vec<_>>();
        u.set_query(None);
        for (k, v) in pairs {
            if !names.contains(&k.as_str()) {
                u.query_pairs_mut().append_pair(&k, &v);
            }
        }
        u.query_pairs_mut().extend_pairs(params.iter().copied());
    }
    Ok(u.into())
}
fn clean(s: String) -> String {
    static TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new("<[^>]*>").unwrap());
    TAG.replace_all(&s, " ")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .chars()
        .take(10000)
        .collect()
}
pub fn playlist(v: &Value) -> Vec<(String, i64, String, i64, String)> {
    let names = text(v, "vod_play_from");
    let names = names.split("$$$").collect::<Vec<_>>();
    let urls = text(v, "vod_play_url");
    let mut out = Vec::new();
    for (li, line) in urls.split("$$$").take(30).enumerate() {
        let mut number = 0;
        for part in line.split('#').filter(|p| !p.trim().is_empty()).take(3000) {
            if let Some((label, url)) = part.split_once('$') {
                if net::valid_url(url.trim()).is_err() {
                    continue;
                }
                number += 1;
                out.push((
                    names
                        .get(li)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("线路 {}", li + 1)),
                    li as i64,
                    if label.trim().is_empty() {
                        format!("第 {number} 集")
                    } else {
                        label.trim().chars().take(200).collect()
                    },
                    number,
                    url.trim().to_owned(),
                ));
            }
        }
    }
    out
}
pub async fn ingest(app: &App, source: &Source, v: &Value, detailed: bool) -> Result<Option<Film>> {
    let vod_id = text(v, "vod_id");
    let title = clean(text(v, "vod_name"));
    if vod_id.is_empty() || title.is_empty() || vod_id.len() > 256 {
        return Ok(None);
    }
    let id = guid(&["film", &source.id, &vod_id]);
    let list = playlist(v);
    let category = text(v, "type_name");
    let max_episodes = list.iter().map(|r| r.3).max().unwrap_or(0);
    let is_tv = max_episodes > 1
        || [
            "电视剧",
            "连续剧",
            "国产剧",
            "日韩剧",
            "欧美剧",
            "港台剧",
            "综艺",
            "动漫",
            "短剧",
        ]
        .iter()
        .any(|s| category.contains(s));
    let kind = if is_tv { "TV" } else { "Movie" };
    let detail_at = if detailed || !list.is_empty() {
        now()
    } else {
        0
    };
    let poster = text(v, "vod_pic");
    let poster = if net::valid_url(&poster).is_ok() {
        poster
    } else {
        String::new()
    };
    let mut tx = app.db.begin().await?;
    sqlx::query("INSERT INTO films(id,source_id,vod_id,title,year,kind,category,poster,overview,remarks,actor,director,updated_at,detailed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET title=excluded.title,year=excluded.year,kind=CASE WHEN excluded.detailed_at>0 THEN excluded.kind ELSE films.kind END,category=excluded.category,poster=CASE WHEN excluded.poster<>'' THEN excluded.poster ELSE films.poster END,overview=CASE WHEN excluded.overview<>'' THEN excluded.overview ELSE films.overview END,remarks=excluded.remarks,actor=excluded.actor,director=excluded.director,updated_at=excluded.updated_at,detailed_at=MAX(films.detailed_at,excluded.detailed_at)")
        .bind(&id).bind(&source.id).bind(&vod_id).bind(&title).bind(text(v,"vod_year")).bind(kind).bind(&category).bind(poster)
        .bind(clean(text(v,"vod_content"))).bind(text(v,"vod_remarks")).bind(clean(text(v,"vod_actor"))).bind(clean(text(v,"vod_director"))).bind(now()).bind(detail_at).execute(&mut *tx).await?;
    if !list.is_empty() || detailed {
        sqlx::query("UPDATE episodes SET active=0 WHERE film_id=?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        for (line, order, label, number, url) in list {
            let eid = guid(&["episode", &id, &line, &label, &number.to_string()]);
            sqlx::query("INSERT INTO episodes(id,film_id,line_name,line_order,title,number,url) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET line_order=excluded.line_order,probed_at=CASE WHEN episodes.url=excluded.url THEN episodes.probed_at ELSE 0 END,probe=CASE WHEN episodes.url=excluded.url THEN episodes.probe ELSE '{}' END,url=excluded.url,active=1")
                .bind(eid).bind(&id).bind(line).bind(order).bind(label).bind(number).bind(url).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(Some(film(app, &id).await?))
}
pub async fn detail(app: &App, id: &str, force: bool) -> Result<Film> {
    let current = film(app, id).await?;
    let s = source(app, &current.source_id).await?;
    if !s.active() || (!force && current.detailed_at > now() - 1800) {
        return Ok(current);
    }
    let u = api_url(&s.api, &[("ac", "detail"), ("ids", &current.vod_id)])?;
    let v = match net::json(app, &u).await {
        Ok(v) if v["list"].is_array() => v,
        _ => {
            net::json(
                app,
                &api_url(&s.api, &[("ac", "videolist"), ("ids", &current.vod_id)])?,
            )
            .await?
        }
    };
    if let Some(row) = v["list"]
        .as_array()
        .and_then(|rows| rows.iter().find(|v| text(v, "vod_id") == current.vod_id))
        && let Some(f) = ingest(app, &s, row, true).await?
    {
        return Ok(f);
    }
    Err(Error::bad("视频源没有返回对应影片详情"))
}
pub async fn search(
    app: &App,
    query: &str,
    page: i64,
    source_filter: Option<&str>,
) -> Result<Value> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 100 || !(1..=500).contains(&page) {
        return Err(Error::bad("关键词或页码无效"));
    }
    let enabled = sources(app)
        .await?
        .into_iter()
        .filter(|s| s.active() && source_filter.is_none_or(|id| s.id == id))
        .collect::<Vec<_>>();
    let key = format!(
        "search:{}",
        hash(format!(
            "{query}|{page}|{}",
            enabled
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ))
    );
    if let Some(v) = app.cache_get(&key).await? {
        return Ok(v);
    }
    let results = stream::iter(enabled.into_iter().map(|s| async move {
        let result = async {
            let url = api_url(
                &s.api,
                &[
                    ("ac", "videolist"),
                    ("wd", query),
                    ("pg", &page.to_string()),
                ],
            )?;
            let v = net::json(app, &url).await?;
            let rows = v["list"]
                .as_array()
                .ok_or_else(|| Error::bad("搜索响应缺少影片列表"))?;
            let mut films = Vec::new();
            for row in rows.iter().take(100) {
                if let Some(f) = ingest(app, &s, row, false).await? {
                    films.push(f);
                }
            }
            Ok::<_, Error>((films, text(&v, "pagecount").parse::<i64>().unwrap_or(1)))
        }
        .await;
        (s, result)
    }))
    .buffer_unordered(12)
    .collect::<Vec<_>>()
    .await;
    let mut items = Vec::new();
    let mut status = Vec::new();
    let mut more = false;
    for (s, r) in results {
        match r {
            Ok((films, pages)) => {
                more |= pages > page;
                status.push(json!({"id":s.id,"name":s.name,"count":films.len(),"ok":true}));
                items.extend(films.into_iter().map(|f| {
                    let key = group_key(&f);
                    let mut v = serde_json::to_value(f).unwrap();
                    v["source_name"] = json!(s.name);
                    v["group_key"] = json!(key);
                    v
                }));
            }
            Err(e) => {
                status.push(json!({"id":s.id,"name":s.name,"count":0,"ok":false,"error":e.1}))
            }
        }
    }
    items.sort_by(|a, b| {
        text(a, "title")
            .cmp(&text(b, "title"))
            .then(text(a, "source_name").cmp(&text(b, "source_name")))
    });
    let result = json!({"items":items,"sources":status,"page":page,"has_more":more});
    app.cache_put(&key, &result, 180).await?;
    Ok(result)
}
pub async fn personal(app: &App, user: &str, kind: &str) -> Result<Vec<Value>> {
    let ids: Vec<String> = if kind == "favorites" {
        sqlx::query_scalar(
            "SELECT film_id FROM favorites WHERE user_id=? ORDER BY created_at DESC LIMIT 1000",
        )
        .bind(user)
        .fetch_all(&app.db)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT film_id FROM progress WHERE user_id=? ORDER BY updated_at DESC LIMIT 1000",
        )
        .bind(user)
        .fetch_all(&app.db)
        .await?
    };
    let mut rows = Vec::new();
    for id in ids {
        rows.push(view(app, user, &id, false).await?);
    }
    Ok(rows)
}
pub async fn view(app: &App, user: &str, id: &str, full: bool) -> Result<Value> {
    let f = film(app, id).await?;
    let s = source(app, &f.source_id).await?;
    let fav: i64 =
        sqlx::query_scalar("SELECT count(*) FROM favorites WHERE user_id=? AND film_id=?")
            .bind(user)
            .bind(id)
            .fetch_one(&app.db)
            .await?;
    let progress:Option<(String,f64,f64,i64)>=sqlx::query_as("SELECT episode_id,position,duration,updated_at FROM progress WHERE user_id=? AND film_id=?").bind(user).bind(id).fetch_optional(&app.db).await?;
    let mut value = serde_json::to_value(&f).unwrap();
    value["source_name"] = json!(s.name);
    value["available"] = json!(s.active());
    value["favorite"] = json!(fav > 0);
    value["progress"]=progress.map(|(episode_id,position,duration,updated_at)|json!({"episode_id":episode_id,"position":position,"duration":duration,"updated_at":updated_at})).unwrap_or(Value::Null);
    if full {
        value["episodes"]=json!(episodes(app,id).await?.into_iter().map(|e|json!({"id":e.id,"title":e.title,"number":e.number,"line_name":e.line_name,"line_order":e.line_order})).collect::<Vec<_>>());
    }
    Ok(value)
}
pub async fn favorite(app: &App, user: &str, id: &str, on: bool) -> Result<()> {
    film(app, id).await?;
    if on {
        sqlx::query("INSERT OR IGNORE INTO favorites VALUES(?,?,?)")
            .bind(user)
            .bind(id)
            .bind(now())
            .execute(&app.db)
            .await?;
    } else {
        sqlx::query("DELETE FROM favorites WHERE user_id=? AND film_id=?")
            .bind(user)
            .bind(id)
            .execute(&app.db)
            .await?;
    }
    Ok(())
}
pub async fn subscription_list(app: &App) -> Result<Value> {
    let rows:Vec<(String,String,String,i64,i64,i64,String)>=sqlx::query_as("SELECT id,name,url,interval_seconds,updated_at,checked_at,error FROM subscriptions ORDER BY name").fetch_all(&app.db).await?;
    Ok(
        json!({"subscriptions":rows.into_iter().map(|(id,name,url,interval_seconds,updated_at,checked_at,error)|json!({"id":id,"name":name,"url":url,"interval_seconds":interval_seconds,"updated_at":updated_at,"checked_at":checked_at,"error":error})).collect::<Vec<_>>(),"sources":sources(app).await?}),
    )
}
pub async fn recommendations(app: &App, kind: &str) -> Result<Value> {
    let kind = if kind == "tv" { "tv" } else { "movie" };
    let key = format!("recommend:{kind}");
    if let Some(v) = app.cache_get(&key).await? {
        return Ok(v);
    }
    let url = format!(
        "https://movie.douban.com/j/search_subjects?type={kind}&tag=%E7%83%AD%E9%97%A8&sort=recommend&page_limit=24&page_start=0"
    );
    let v = net::json(app, &url).await?;
    let list = v["subjects"]
        .as_array()
        .ok_or_else(|| Error::bad("推荐服务暂不可用，仍可使用搜索和收藏"))?;
    let result = json!({"items":list.iter().map(|v|json!({"id":text(v,"id"),"title":text(v,"title"),"cover":text(v,"cover"),"rate":text(v,"rate")})).collect::<Vec<_>>()});
    app.cache_put(&key, &result, 21600).await?;
    Ok(result)
}
pub async fn maintenance(app: App) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let due: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM subscriptions WHERE url<>'' AND checked_at+interval_seconds<?",
        )
        .bind(now())
        .fetch_all(&app.db)
        .await
        .unwrap_or_default();
        for id in due {
            if let Err(e) = refresh(&app, &id).await {
                tracing::warn!(subscription=%id,reason=%e.1,"订阅更新失败，继续使用已有配置");
            }
        }
        let _ = sqlx::query("DELETE FROM auth_sessions WHERE expires_at<?")
            .bind(now())
            .execute(&app.db)
            .await;
        let _ = sqlx::query("DELETE FROM cache WHERE expires_at<?")
            .bind(now() - 86400)
            .execute(&app.db)
            .await;
        let _ = sqlx::query("DELETE FROM plays WHERE started_at<?")
            .bind(now() - 30 * 86400)
            .execute(&app.db)
            .await;
    }
}
