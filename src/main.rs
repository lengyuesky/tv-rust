use std::path::PathBuf;
use tv_rust::{auth, catalog, core::App};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let dir = PathBuf::from(std::env::var("TV_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let app = App::new(
        dir,
        std::env::var("TV_ALLOW_PRIVATE_UPSTREAM").is_ok_and(|v| v == "1"),
        std::env::var("TV_PUBLIC_URL").unwrap_or_default(),
    )
    .await?;
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|s| s == "reset-password") {
        let username = args.get(2).ok_or_else(|| {
            anyhow::anyhow!("用法：tv-rust reset-password 用户名；新密码从标准输入读取")
        })?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        auth::reset_password(&app, username, input.trim_end_matches(['\r', '\n'])).await?;
        println!("密码已重置，该用户的旧登录和播放会话已撤销");
        return Ok(());
    }
    if args.get(1).is_some_and(|s| s == "backup") {
        let path = args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("用法：tv-rust backup 新备份文件.sqlite"))?;
        sqlx::query("VACUUM INTO ?")
            .bind(path)
            .execute(&app.db)
            .await?;
        println!("一致性数据库备份已创建");
        return Ok(());
    }
    let address = std::env::var("TV_BIND").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let web = std::env::var("TV_WEB_DIR").unwrap_or_else(|_| "web/dist".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(address=%listener.local_addr()?,"TV Rust 已启动；首次使用请在网页设置管理员并导入订阅");
    tokio::spawn(catalog::maintenance(app.clone()));
    axum::serve(listener, tv_rust::router(app, &web))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
