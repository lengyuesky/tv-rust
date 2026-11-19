#![recursion_limit = "256"]

pub mod access;
pub mod api;
pub mod auth;
pub mod catalog;
pub mod core;
pub mod fntv;
pub mod net;
pub mod playback;
pub mod preferences;
pub mod probe;

use axum::{
    Router,
    extract::{Path, Request, State},
    routing::any,
};
use core::{App, Error};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(app: App, web_dir: &str) -> Router {
    Router::new()
        .route("/api/{*path}", any(api::handle))
        .route("/trimcon", any(fntv::handle))
        .route("/v/{*path}", any(fntv::handle))
        .route("/tv-poster/{*path}", any(fntv::handle))
        .route("/media/{access}/{asset}", any(media))
        .fallback_service(
            ServeDir::new(web_dir)
                .not_found_service(ServeFile::new(format!("{web_dir}/index.html"))),
        )
        .layer(axum::middleware::from_fn(
            |req: Request, next: axum::middleware::Next| async move {
                let path = if req.uri().path().starts_with("/media/") {
                    "/media/{access}/{asset}".to_owned()
                } else {
                    req.uri()
                        .path()
                        .split('/')
                        .take(6)
                        .collect::<Vec<_>>()
                        .join("/")
                };
                let method = req.method().clone();
                let mut r = next.run(req).await;
                if r.status().is_client_error() || r.status().is_server_error() {
                    tracing::warn!(%method,%path,status=r.status().as_u16(),"请求未成功");
                }
                r.headers_mut()
                    .insert("x-content-type-options", "nosniff".parse().unwrap());
                r.headers_mut()
                    .insert("referrer-policy", "no-referrer".parse().unwrap());
                r.headers_mut()
                    .insert("x-frame-options", "DENY".parse().unwrap());
                r
            },
        ))
        .with_state(app)
}
async fn media(
    State(app): State<App>,
    Path((access, asset)): Path<(String, String)>,
    req: Request,
) -> core::Result<axum::response::Response> {
    if !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) {
        return Err(Error::bad("媒体仅支持 GET/HEAD"));
    }
    playback::proxy(
        &app,
        &access,
        &asset,
        req.method().clone(),
        req.headers().clone(),
    )
    .await
}
