//! 所有服务端远端请求经过同一校验；逐次解析并固定地址，避免重定向和 DNS 重绑定绕过。
use crate::core::*;
use axum::http::{HeaderMap, Method};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

type ClientKey = (String, u16, Vec<SocketAddr>);

#[derive(Default)]
pub(crate) struct Clients(Mutex<HashMap<ClientKey, (Instant, reqwest::Client)>>);

impl Clients {
    fn get(&self, host: &str, port: u16, addrs: &[SocketAddr]) -> Result<reqwest::Client> {
        // 调用方每次先重新解析、验证地址；只有 DNS 结果相同时才复用固定地址的连接池。
        let mut addresses = addrs.to_vec();
        addresses.sort_unstable();
        addresses.dedup();
        let key = (host.to_owned(), port, addresses);
        let mut clients = self.0.lock().unwrap();
        clients.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        if let Some((at, client)) = clients.get_mut(&key) {
            *at = Instant::now();
            return Ok(client.clone());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, addrs)
            .connect_timeout(Duration::from_secs(7))
            .read_timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .user_agent("TV-Rust/0.1 (+source-subscription)")
            .build()
            .map_err(|_| Error::bad("无法建立上游连接"))?;
        if clients.len() >= 32
            && let Some(oldest) = clients
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
        {
            clients.remove(&oldest);
        }
        clients.insert(key, (Instant::now(), client.clone()));
        Ok(client)
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        && ip.octets()[0] != 0
        && ip.octets()[0] < 240
}
fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_unique_local()
        && !ip.is_unicast_link_local()
        && (ip.segments()[0] & 0xe000) == 0x2000
}
pub fn valid_url(s: &str) -> Result<Url> {
    let u = Url::parse(s).map_err(|_| Error::bad("地址格式无效"))?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || s.len() > 8192
    {
        return Err(Error::bad("只支持不含登录凭据的 HTTP/HTTPS 地址"));
    }
    Ok(u)
}
pub async fn request(
    app: &App,
    method: Method,
    input: &str,
    headers: &HeaderMap,
) -> Result<reqwest::Response> {
    let mut url = valid_url(input)?;
    for _ in 0..6 {
        let host = url.host_str().unwrap().trim_matches(['[', ']']).to_owned();
        let port = url.port_or_known_default().unwrap();
        let addrs = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| Error::bad("源域名解析超时"))?
        .map_err(|_| Error::bad("源域名解析失败"))?
        .collect::<Vec<_>>();
        if addrs.is_empty()
            || (!app.allow_private
                && addrs.iter().any(|a| !match a.ip() {
                    IpAddr::V4(v) => public_v4(v),
                    IpAddr::V6(v) => public_v6(v),
                }))
        {
            return Err(Error::denied(
                "上游地址指向受限网络；内网源需在部署时显式启用 TV_ALLOW_PRIVATE_UPSTREAM",
            ));
        }
        let client = app.upstream_clients.get(&host, port, &addrs)?;
        let mut attempt = 0;
        let started = Instant::now();
        let response = loop {
            let mut req = client.request(method.clone(), url.clone());
            for key in [
                "range",
                "if-range",
                "if-none-match",
                "if-modified-since",
                "accept",
            ] {
                if let Some(v) = headers.get(key) {
                    req = req.header(key, v);
                }
            }
            match req.send().await {
                Ok(response) => break response,
                Err(error)
                    if attempt == 0
                        && matches!(method, Method::GET | Method::HEAD)
                        && (error.is_connect() || error.is_timeout())
                        && started.elapsed() < Duration::from_secs(8) =>
                {
                    tracing::debug!(host, port, error = %error.without_url(), "上游连接失败，重试一次");
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                Err(error) => {
                    tracing::warn!(host, port, error = %error.without_url(), "上游请求失败");
                    return Err(Error::bad("上游连接、读取或证书验证失败"));
                }
            }
        };
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::bad("上游重定向缺少地址"))?;
            url = url
                .join(location)
                .map_err(|_| Error::bad("上游重定向地址无效"))?;
            valid_url(url.as_str())?;
            continue;
        }
        return Ok(response);
    }
    Err(Error::bad("上游重定向次数过多"))
}
pub async fn bytes(response: reqwest::Response, limit: usize, truncate: bool) -> Result<Vec<u8>> {
    let mut body = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| Error::bad("上游响应未完整读取"))?;
        if out.len() + chunk.len() > limit {
            if !truncate {
                return Err(Error::bad("上游响应超过大小限制"));
            }
            out.extend_from_slice(&chunk[..limit - out.len()]);
            break;
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
pub async fn json(app: &App, url: &str) -> Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let r = request(app, Method::GET, url, &HeaderMap::new()).await?;
        if !r.status().is_success() {
            return Err(Error::bad(format!("上游返回 HTTP {}", r.status().as_u16())));
        }
        let data = bytes(r, 4 * 1024 * 1024, false).await?;
        serde_json::from_slice(&data).map_err(|_| Error::bad("源返回的内容不是有效 JSON"))
    })
    .await
    .map_err(|_| Error::bad("视频源响应超时"))?
}
