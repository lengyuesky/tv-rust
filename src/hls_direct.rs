//! 为图片头伪装的 TS 分片识别直读区间，不交付或转发媒体字节。
use crate::{core::*, net};
use axum::http::{HeaderMap, Method};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SegmentRange {
    pub length: u64,
    pub offset: u64,
}

pub(crate) fn image_segment(url: &url::Url) -> bool {
    matches!(
        url.path()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif"
    )
}

async fn inspect(app: &App, url: &str) -> Result<SegmentRange> {
    let key = format!("direct-ts:v1:{}", hash(url));
    if let Some(value) =
        sqlx::query_scalar::<_, String>("SELECT value FROM cache WHERE key=? AND expires_at>?")
            .bind(&key)
            .bind(now())
            .fetch_optional(&app.db)
            .await?
        && let Ok(value) = serde_json::from_str(&value)
    {
        return Ok(value);
    }
    let mut headers = HeaderMap::new();
    headers.insert("range", "bytes=0-4095".parse().unwrap());
    let response = net::request(app, Method::GET, url, &headers).await?;
    let range = response
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if response.status().as_u16() != 206 || !range.starts_with("bytes 0-") {
        return Err(Error::bad(
            "当前来源不支持直接读取视频区间，请换源或允许必要转发",
        ));
    }
    let total: u64 = range
        .rsplit('/')
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::bad("无法确认当前来源的视频区间，请换源"))?;
    let bytes = net::bytes(response, 4096, true).await?;
    let offset = bytes
        .windows(188 * 4 + 1)
        .position(|bytes| (0..5).all(|index| bytes[index * 188] == 0x47))
        .ok_or_else(|| Error::bad("当前分片未返回可直接播放的视频，请换源"))?
        as u64;
    let length = total
        .checked_sub(offset)
        .filter(|length| *length > 0)
        .ok_or_else(|| Error::bad("当前来源的视频区间无效，请换源"))?;
    let value = SegmentRange { length, offset };
    sqlx::query("INSERT OR REPLACE INTO cache(key,value,expires_at) VALUES(?,?,?)")
        .bind(key)
        .bind(serde_json::to_string(&value).unwrap())
        .bind(now() + 3600)
        .execute(&app.db)
        .await?;
    Ok(value)
}

pub(crate) async fn ranges(
    app: &App,
    base: &url::Url,
    input: &str,
) -> Result<BTreeMap<String, SegmentRange>> {
    let mut urls = BTreeMap::new();
    let mut playlist = false;
    let mut ranged = false;
    let mut encrypted = false;
    for line in input.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if let Some(key) = line.strip_prefix("#EXT-X-KEY:") {
            encrypted = !key.split(',').any(|part| part.trim() == "METHOD=NONE");
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            playlist = true;
        } else if line.starts_with("#EXT-X-BYTERANGE:") {
            ranged = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            let url = base
                .join(line)
                .map_err(|_| Error::bad("HLS 资源地址无效"))?;
            if !playlist && !ranged && !encrypted && image_segment(&url) {
                net::valid_url(url.as_str())?;
                urls.insert(url.to_string(), ());
            }
            playlist = false;
            ranged = false;
        }
    }
    if urls.len() > 12000 {
        return Err(Error::bad("此清单的媒体资源数量超出上限，请换源"));
    }
    let mut pending = stream::iter(urls.into_keys().map(|url| async move {
        let range = inspect(app, &url).await?;
        Ok::<_, Error>((url, range))
    }))
    .buffer_unordered(64);
    let mut result = BTreeMap::new();
    while let Some(range) = pending.next().await {
        let (url, range) = range?;
        result.insert(url, range);
    }
    Ok(result)
}
