use crate::{
    catalog::{self, Episode},
    core::*,
    net,
};
use axum::http::{HeaderMap, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Seek, SeekFrom, Write},
    time::Duration,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    #[serde(default)]
    pub probe_version: u8,
    pub kind: String,
    pub duration: f64,
    pub size: u64,
    pub streams: Vec<Value>,
    pub warning: String,
}
pub fn is_hls(data: &[u8]) -> bool {
    String::from_utf8_lossy(&data[..data.len().min(256)])
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
}
fn total_size(headers: &HeaderMap) -> u64 {
    headers
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next())
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
}
async fn chunk(app: &App, url: &str, range: &str) -> Result<(Vec<u8>, String, HeaderMap)> {
    let mut headers = HeaderMap::new();
    headers.insert("range", range.parse().unwrap());
    let r = net::request(app, Method::GET, url, &headers).await?;
    if !r.status().is_success() {
        return Err(Error::bad(format!(
            "媒体探测返回 HTTP {}",
            r.status().as_u16()
        )));
    }
    let url = r.url().to_string();
    let headers = r.headers().clone();
    Ok((net::bytes(r, 512 * 1024, true).await?, url, headers))
}
async fn local_probe(prefix: Vec<u8>, tail: Option<(u64, Vec<u8>)>) -> Result<Value> {
    // 部分 HLS 分片前置 PNG 图片头，自动识别会只看到小图片。
    // 连续五个 TS 同步字节确认媒体容器后，交给 MPEG-TS 解复用器识别实际视频。
    let image_ts = prefix.starts_with(b"\x89PNG\r\n\x1a\n")
        && prefix
            .windows(188 * 4 + 1)
            .any(|data| (0..5).all(|index| data[index * 188] == 0x47));
    let mut file = tempfile::Builder::new()
        .prefix("tv-probe-")
        .suffix(".media")
        .tempfile()?;
    file.write_all(&prefix)?;
    if let Some((offset, bytes)) = tail {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&bytes)?;
    }
    file.flush()?;
    let value = probe_file(file.path(), "file", image_ts.then_some("mpegts")).await?;
    let format = text(&value["format"], "format_name");
    if matches!(
        format.as_str(),
        "png_pipe" | "jpeg_pipe" | "gif" | "webp_pipe" | "bmp_pipe" | "image2" | "image2pipe"
    ) {
        return Err(Error::bad("媒体地址或分片返回图片，请刷新来源或换源"));
    }
    Ok(value)
}
async fn probe_file(
    path: &std::path::Path,
    protocols: &str,
    format: Option<&str>,
) -> Result<Value> {
    // 仅探测已经经过网络校验的有界本地数据，ffprobe 不接触远端 URL。
    let mut command = tokio::process::Command::new("ffprobe");
    if protocols == "file,crypto" {
        command.args(["-allowed_extensions", "key,ts,m3u8"]);
    }
    if let Some(format) = format {
        command.args(["-f", format]);
    }
    let output = tokio::time::timeout(
        Duration::from_secs(6),
        command
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                protocols,
                "-analyzeduration",
                "1500000",
                "-probesize",
                "524288",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(path)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::bad("媒体元数据探测超时"))??;
    if output.stdout.len() > 512 * 1024 {
        return Err(Error::bad("媒体元数据过大"));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| Error::bad("未识别到媒体元数据"))
}
async fn encrypted_hls_probe(app: &App, base: &str, manifest: &str) -> Result<Value> {
    static URI: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"(?:^|,)URI="([^"]+)""#).unwrap());
    let key = manifest
        .lines()
        .find_map(|s| s.trim().strip_prefix("#EXT-X-KEY:"))
        .ok_or_else(|| Error::bad("HLS 缺少密钥信息"))?;
    if !key.split(',').any(|s| s == "METHOD=AES-128")
        || key.contains("KEYFORMAT=") && !key.contains("KEYFORMAT=\"identity\"")
    {
        return Err(Error::bad("此加密格式不支持有界元数据探测"));
    }
    let key_uri = URI
        .captures(key)
        .and_then(|c| c.get(1))
        .ok_or_else(|| Error::bad("HLS 密钥地址无效"))?;
    let base = net::valid_url(base)?;
    let key_url = base
        .join(key_uri.as_str())
        .map_err(|_| Error::bad("HLS 密钥地址无效"))?;
    let response = net::request(app, Method::GET, key_url.as_str(), &HeaderMap::new()).await?;
    if !response.status().is_success() {
        return Err(Error::bad("HLS 密钥获取失败"));
    }
    let key_bytes = net::bytes(response, 16, false).await?;
    if key_bytes.len() != 16 {
        return Err(Error::bad("AES-128 密钥长度无效"));
    }
    let segment = manifest
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.starts_with('#'))
        .ok_or_else(|| Error::bad("HLS 缺少分片"))?;
    let segment_url = base
        .join(segment)
        .map_err(|_| Error::bad("HLS 分片地址无效"))?;
    let (data, _, _) = chunk(app, segment_url.as_str(), "bytes=0-524287").await?;
    let iv = key.split(',').find_map(|s| s.strip_prefix("IV="));
    if iv.is_some_and(|s| {
        s.len() != 34 || !s.starts_with("0x") || !s[2..].bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        return Err(Error::bad("HLS 初始化向量无效"));
    }
    let sequence = manifest
        .lines()
        .find_map(|s| s.trim().strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let dir = tempfile::Builder::new().prefix("tv-hls-probe-").tempdir()?;
    std::fs::write(dir.path().join("key.key"), key_bytes)?;
    std::fs::write(dir.path().join("segment.ts"), data)?;
    let iv = iv.map(|s| format!(",IV={s}")).unwrap_or_default();
    // 只将本站构造的本地路径交给 ffprobe，远端密钥与分片仍经过统一网络校验。
    let local = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.key\"{iv}\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n"
    );
    let path = dir.path().join("probe.m3u8");
    std::fs::write(&path, local)?;
    probe_file(&path, "file,crypto", None).await
}
fn media_duration(v: &Value) -> f64 {
    text(&v["format"], "duration")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
}
async fn inspect(app: &App, e: &Episode) -> Result<MediaInfo> {
    let (prefix, mut base, headers) = chunk(app, &e.url, "bytes=0-524287").await?;
    let mut info = MediaInfo {
        probe_version: 2,
        size: total_size(&headers),
        ..Default::default()
    };
    if is_hls(&prefix) {
        info.kind = "hls".into();
        info.size = 0;
        let mut manifest = String::from_utf8(prefix).map_err(|_| Error::bad("HLS 清单编码无效"))?;
        for _ in 0..4 {
            if !manifest.contains("#EXT-X-STREAM-INF:") {
                break;
            }
            let next = manifest
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty() && !s.starts_with('#'))
                .ok_or_else(|| Error::bad("HLS 主清单没有媒体地址"))?;
            let url = net::valid_url(&base)?
                .join(next)
                .map_err(|_| Error::bad("HLS 地址无效"))?;
            let (data, new_base, _) = chunk(app, url.as_str(), "bytes=0-524287").await?;
            if !is_hls(&data) {
                return Err(Error::bad("HLS 子清单无效"));
            }
            base = new_base;
            manifest = String::from_utf8_lossy(&data).into_owned();
        }
        info.duration = manifest
            .lines()
            .filter_map(|line| line.strip_prefix("#EXTINF:"))
            .filter_map(|s| s.split(',').next()?.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0)
            .sum();
        if manifest.contains("#EXT-X-KEY:METHOD=AES-128")
            && let Ok(v) = encrypted_hls_probe(app, &base, &manifest).await
        {
            info.streams = v["streams"].as_array().cloned().unwrap_or_default();
        }
        if (!manifest.contains("#EXT-X-KEY:") || manifest.contains("METHOD=NONE"))
            && let Some(segment) = manifest
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty() && !s.starts_with('#'))
        {
            let mut data = Vec::new();
            if let Some(map) = manifest.lines().find(|s| s.starts_with("#EXT-X-MAP:"))
                && let Some(uri) = map.split("URI=\"").nth(1).and_then(|s| s.split('"').next())
                && let Ok(url) = net::valid_url(&base)?.join(uri)
                && let Ok((b, _, _)) = chunk(app, url.as_str(), "bytes=0-131071").await
            {
                data.extend(b);
            }
            if let Ok(url) = net::valid_url(&base)?.join(segment)
                && let Ok((b, _, _)) = chunk(app, url.as_str(), "bytes=0-524287").await
            {
                data.extend(b);
            }
            match local_probe(data, None).await {
                Ok(v) => info.streams = v["streams"].as_array().cloned().unwrap_or_default(),
                Err(error) if error.1.contains("返回图片") => info.warning = error.1,
                _ => {}
            }
        }
    } else {
        let head = String::from_utf8_lossy(&prefix[..prefix.len().min(128)])
            .trim()
            .to_ascii_lowercase();
        if head.starts_with("<!doctype html") || head.starts_with("<html") {
            return Err(Error::bad("该地址返回网页播放器，需要额外解析，请换源"));
        }
        info.kind = if prefix.get(4..8) == Some(b"ftyp") {
            "mp4"
        } else {
            "video"
        }
        .into();
        let mut tail = None;
        if info.size > 512 * 1024 && info.size < 128 * 1024 * 1024 * 1024 {
            let offset = info.size - 512 * 1024;
            if let Ok((bytes, _, h)) = chunk(app, &e.url, &format!("bytes={offset}-")).await {
                let expected = format!("bytes {offset}-");
                if h.get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|s| s.starts_with(&expected))
                {
                    tail = Some((offset, bytes));
                }
            }
        }
        match local_probe(prefix, tail).await {
            Ok(v) => {
                info.duration = media_duration(&v);
                info.streams = v["streams"].as_array().cloned().unwrap_or_default();
            }
            Err(error) if error.1.contains("返回图片") => return Err(error),
            _ => {}
        }
    }
    Ok(info)
}
pub(crate) fn cached(e: &Episode) -> Option<MediaInfo> {
    let info = serde_json::from_str::<MediaInfo>(&e.probe).ok()?;
    let image_only = info
        .streams
        .iter()
        .any(|video| video["codec_name"] == "png")
        && !info
            .streams
            .iter()
            .any(|video| is_video_stream(video) && video["codec_name"] != "png");
    let unrecognized_hls = info.kind != "hls"
        && info.streams.is_empty()
        && (e.line_name.to_ascii_lowercase().contains("m3u8")
            || net::valid_url(&e.url)
                .ok()
                .is_some_and(|url| url.path().to_ascii_lowercase().ends_with(".m3u8")));
    (e.probed_at > now() - 21600
        && !(info.kind == "hls" && info.streams.is_empty() && info.probe_version == 0)
        && !(image_only && info.probe_version < 2)
        && !unrecognized_hls)
        .then_some(info)
}

pub async fn get(app: &App, e: &Episode) -> Result<MediaInfo> {
    get_inner(app, e, false).await
}

pub(crate) fn require_available(info: &MediaInfo) -> Result<()> {
    if matches!(
        info.warning.as_str(),
        "媒体探测返回 HTTP 404" | "媒体探测返回 HTTP 410"
    ) {
        return Err(Error::bad("媒体地址已失效，请刷新来源或选择其他播放源"));
    }
    if info.warning == "媒体地址或分片返回图片，请刷新来源或换源" {
        return Err(Error::bad(&info.warning));
    }
    if info.kind == "hls"
        && let Some(status) = info
            .warning
            .strip_prefix("媒体探测返回 HTTP ")
            .and_then(|status| status.parse::<u16>().ok())
        && (400..500).contains(&status)
        && !matches!(status, 408 | 429)
    {
        return Err(Error::bad(format!(
            "HLS 清单被上游拒绝（HTTP {status}），请刷新来源或换源"
        )));
    }
    Ok(())
}

pub async fn source_quality(
    app: &App,
    user: &str,
    e: &Episode,
    refresh: bool,
    selected: bool,
) -> Result<MediaInfo> {
    let fresh = catalog::available(app, user, &e.id).await?.0;
    if !refresh && let Some(info) = cached(&fresh) {
        return Ok(info);
    }
    // 后台来源识别最多占用一个探测槽，保留另一个供网页和飞牛开始播放。
    let _background = if selected {
        None
    } else {
        Some(
            app.source_quality_slots
                .acquire()
                .await
                .map_err(|_| Error::bad("来源画质识别已停止"))?,
        )
    };
    let fresh = catalog::available(app, user, &e.id).await?.0;
    let info = get_inner(app, &fresh, refresh).await?;
    catalog::available(app, user, &e.id).await?;
    Ok(info)
}

async fn get_inner(app: &App, e: &Episode, refresh: bool) -> Result<MediaInfo> {
    if !refresh && let Some(info) = cached(e) {
        return Ok(info);
    }
    let _slot = app
        .probe_slots
        .acquire()
        .await
        .map_err(|_| Error::bad("媒体探测已停止"))?;
    let fresh = catalog::episode(app, &e.id).await?;
    if !refresh && let Some(v) = cached(&fresh) {
        return Ok(v);
    }
    let result = tokio::time::timeout(Duration::from_secs(14), inspect(app, &fresh)).await;
    let info = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) if e.1.contains("网页播放器") || e.1.contains("返回图片") => {
            return Err(e);
        }
        other => {
            let warning = match other {
                Ok(Err(e)) => e.1,
                _ => "服务端探测超时，仍尝试客户端直连".into(),
            };
            let path = net::valid_url(&fresh.url)?.path().to_lowercase();
            MediaInfo {
                probe_version: 2,
                kind: if path.ends_with(".m3u8") || fresh.line_name.to_lowercase().contains("m3u8")
                {
                    "hls"
                } else {
                    "video"
                }
                .into(),
                warning,
                ..Default::default()
            }
        }
    };
    let saved =
        sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=? AND url=? AND active=1")
            .bind(serde_json::to_string(&info).unwrap())
            .bind(now())
            .bind(&e.id)
            .bind(&fresh.url)
            .execute(&app.db)
            .await?;
    if saved.rows_affected() == 0 {
        return Err(Error::bad("播放地址已更新，请重新识别画质"));
    }
    Ok(info)
}
pub fn video(info: &MediaInfo) -> Value {
    info.streams.iter().find(|v|is_video_stream(v)).cloned().unwrap_or(json!({"index":0,"codec_type":"video","codec_name":"h264","width":0,"height":0,"level":0,"pix_fmt":"yuv420p"}))
}

pub(crate) fn is_video_stream(stream: &Value) -> bool {
    stream["codec_type"] == "video"
        && stream["disposition"]["attached_pic"] != 1
        && stream["disposition"]["attached_pic"] != true
}

pub fn quality(info: &MediaInfo) -> Value {
    // 网页只展示实际探测字段，不采用协议适配中的缺省 H.264 占位。
    let video = info.streams.iter().find(|v| is_video_stream(v));
    let number = |key: &str| {
        video
            .and_then(|v| match &v[key] {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })
            .filter(|n| {
                n.is_finite() && *n > 0.0 && (!matches!(key, "width" | "height") || *n <= 32768.0)
            })
            .unwrap_or(0.0)
    };
    let rate = |key: &str| {
        let raw = video.map(|v| text(v, key)).unwrap_or_default();
        let fps = if let Some((a, b)) = raw.split_once('/') {
            a.parse::<f64>()
                .ok()
                .zip(b.parse::<f64>().ok())
                .map(|(a, b)| a / b)
        } else {
            raw.parse::<f64>().ok()
        };
        fps.filter(|n| n.is_finite() && *n > 0.0 && *n <= 1000.0)
            .unwrap_or(0.0)
    };
    let average = rate("avg_frame_rate");
    json!({"width":number("width"),"height":number("height"),
        "codec":video.map(|v|text(v,"codec_name")).unwrap_or_default(),
        "frame_rate":if average>0.0 {average} else {rate("r_frame_rate")},
        "bitrate":number("bit_rate")})
}
