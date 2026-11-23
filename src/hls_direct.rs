//! 为图片头伪装的 TS 分片识别直读区间，不交付或转发媒体字节。
use crate::{core::*, net};
use axum::http::{HeaderMap, Method};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SegmentRange {
    pub length: u64,
    pub offset: u64,
}

pub(crate) struct RangeJob {
    manifest: String,
    finished: Preparation<BTreeMap<String, SegmentRange>>,
}

struct Identifying(App, String);
impl Drop for Identifying {
    fn drop(&mut self) {
        self.0.direct_range_jobs.lock().unwrap().remove(&self.1);
    }
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
    let _slot = app.direct_range_slots.acquire().await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("range", "bytes=0-4095".parse().unwrap());
    let mut response = net::request(app, Method::GET, url, &headers).await?;
    if matches!(
        response.status().as_u16(),
        408 | 429 | 500 | 502 | 503 | 504
    ) {
        let wait = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if wait <= 2 {
            net::bytes(response, 4096, true).await?;
            tokio::time::sleep(Duration::from_millis((wait * 1000).max(150))).await;
            response = net::request(app, Method::GET, url, &headers).await?;
        }
    }
    if !response.status().is_success() {
        return Err(Error::bad(format!(
            "上游分片返回 HTTP {}，请重试或换源",
            response.status().as_u16()
        )));
    }
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
    Ok(SegmentRange { length, offset })
}

async fn save_ranges(app: &App, ranges: &[(String, SegmentRange)]) -> Result<()> {
    if ranges.is_empty() {
        return Ok(());
    }
    let expires = now() + 86400;
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    for chunk in ranges.chunks(250) {
        let mut query = sqlx::QueryBuilder::new("INSERT INTO cache(key,value,expires_at) ");
        query.push_values(chunk, |mut row, (url, range)| {
            row.push_bind(format!("direct-ts:v1:{}", hash(url)))
                .push_bind(serde_json::to_string(range).unwrap())
                .push_bind(expires);
        });
        query.push(
            " ON CONFLICT(key) DO UPDATE SET value=excluded.value,expires_at=excluded.expires_at",
        );
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn ranges(
    app: &App,
    base: &url::Url,
    input: &str,
) -> Result<BTreeMap<String, SegmentRange>> {
    let key = hash(base.as_str());
    let manifest = hash(input);
    loop {
        let (pending, same_manifest) = {
            let mut jobs = app.direct_range_jobs.lock().unwrap();
            if let Some(job) = jobs.get(&key) {
                (job.finished.clone(), job.manifest == manifest)
            } else {
                let (finished, pending) = tokio::sync::watch::channel(None);
                jobs.insert(
                    key.clone(),
                    RangeJob {
                        manifest: manifest.clone(),
                        finished: pending.clone(),
                    },
                );
                let guard = Identifying(app.clone(), key.clone());
                let (app, base, input) = (app.clone(), base.clone(), input.to_owned());
                // 请求结束只停止等待；同一清单共享识别，已确认区间继续保存。
                tokio::spawn(async move {
                    let result = identify_ranges(&app, &base, &input).await;
                    drop(guard);
                    finished.send_replace(Some(Arc::new(result)));
                });
                (pending, true)
            }
        };
        let result = await_preparation(pending).await;
        if same_manifest {
            return result;
        }
        // 同址清单更新时等旧任务退出，再按新内容读取缓存，不能复用旧清单结果。
    }
}

async fn identify_ranges(
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
    let keys: Vec<_> = urls
        .into_keys()
        .map(|url| (format!("direct-ts:v1:{}", hash(&url)), url))
        .collect();
    let mut cached = BTreeMap::<String, SegmentRange>::new();
    for chunk in keys.chunks(500) {
        let mut query = sqlx::QueryBuilder::new("SELECT key,value FROM cache WHERE expires_at>");
        query.push_bind(now()).push(" AND key IN (");
        let mut fields = query.separated(",");
        for (key, _) in chunk {
            fields.push_bind(key);
        }
        fields.push_unseparated(")");
        for (key, value) in query
            .build_query_as::<(String, String)>()
            .fetch_all(&app.db)
            .await?
        {
            if let Ok(range) = serde_json::from_str(&value) {
                cached.insert(key, range);
            }
        }
    }
    let mut result = BTreeMap::new();
    let mut missing = Vec::new();
    for (key, url) in keys {
        if let Some(range) = cached.remove(&key) {
            result.insert(url, range);
        } else {
            missing.push(url);
        }
    }
    let mut pending = stream::iter(missing.into_iter().map(|url| async move {
        let range = inspect(app, &url)
            .await
            .map_err(|error| Error(error.0, format!("HLS 分片区间识别失败：{}", error.1)))?;
        Ok::<_, Error>((url, range))
    }))
    .buffer_unordered(32);
    let mut identified = Vec::new();
    let mut failure = None;
    while let Some(range) = pending.next().await {
        match range {
            Ok((url, range)) => {
                identified.push((url, range));
                if identified.len() == 64 {
                    save_ranges(app, &identified).await?;
                    result.extend(identified.drain(..));
                }
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    drop(pending);
    // 每批提交避免逐分片写事务；清单失败时也保存最后一批，重试只识别缺失部分。
    save_ranges(app, &identified).await?;
    if let Some(error) = failure {
        return Err(error);
    }
    result.extend(identified);
    Ok(result)
}
