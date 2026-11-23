use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Semaphore, watch};

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) type Preparation<T> = watch::Receiver<Option<Arc<Result<T>>>>;

pub(crate) async fn await_preparation<T: Clone>(mut pending: Preparation<T>) -> Result<T> {
    loop {
        if let Some(result) = pending.borrow().clone() {
            return result
                .as_ref()
                .as_ref()
                .cloned()
                .map_err(|error| Error(error.0, error.1.clone()));
        }
        pending
            .changed()
            .await
            .map_err(|_| Error::bad("后台直链准备已中断，请重试"))?;
    }
}

#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);
impl Error {
    pub fn bad(s: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, s.into())
    }
    pub fn denied(s: impl Into<String>) -> Self {
        Self(StatusCode::FORBIDDEN, s.into())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "内容不存在".into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "请重新登录".into())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.1)
    }
}
impl std::error::Error for Error {}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "数据库操作失败");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "数据库操作失败".into())
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("文件操作失败：{}", e.kind()),
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":self.1,"code":self.0.as_u16(),"msg":self.1,"data":null})),
        )
            .into_response()
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn build_revision() -> &'static str {
    option_env!("TV_BUILD_REVISION").unwrap_or("development")
}
pub fn hash(v: impl AsRef<[u8]>) -> String {
    Sha256::digest(v.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn random() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn guid(parts: &[&str]) -> String {
    let h = hash(serde_json::to_vec(parts).unwrap());
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}
pub fn text(v: &serde_json::Value, key: &str) -> String {
    match &v[key] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[derive(Clone)]
pub struct App(pub Arc<Inner>);
pub struct Inner {
    pub db: SqlitePool,
    pub data_dir: PathBuf,
    pub forbid_proxy: watch::Sender<bool>,
    pub access_changes: watch::Sender<u64>,
    pub allow_private: bool,
    pub public_url: String,
    pub probe_slots: Semaphore,
    /// 起播预检的并发上限。预检会在后台跑完整张清单的区间识别，单独计数，
    /// 不占用画质探测名额，否则慢清单会让起播时的探测排队十几秒。
    pub(crate) direct_ready_slots: Semaphore,
    pub direct_range_slots: Semaphore,
    pub(crate) direct_range_jobs: Mutex<HashMap<String, crate::hls_direct::RangeJob>>,
    pub(crate) direct_ready_jobs: Mutex<HashMap<String, Preparation<()>>>,
    pub(crate) upstream_clients: crate::net::Clients,
    pub source_quality_slots: Semaphore,
    /// 正在后台识别画质的单集。识别不随发起它的请求结束，同一单集只识别一次。
    pub(crate) probing: Mutex<std::collections::HashSet<String>>,
    pub recommendation_slots: Semaphore,
    pub login_budget: Mutex<HashMap<String, (i64, usize)>>,
    pub refresh_lock: tokio::sync::Mutex<()>,
    pub native_switch_lock: tokio::sync::Mutex<()>,
}
impl std::ops::Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}
impl App {
    pub async fn new(dir: PathBuf, allow_private: bool, public_url: String) -> Result<Self> {
        tokio::fs::create_dir_all(&dir).await?;
        let db = SqlitePoolOptions::new()
            .max_connections(6)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.join("tv.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0001.sql"))
            .execute(&db)
            .await?;
        let schema: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&db)
            .await?;
        if schema < 2 {
            let mut tx = db.begin().await?;
            sqlx::raw_sql(include_str!("../migrations/0002.sql"))
                .execute(&mut *tx)
                .await?;
            // 旧版移除会清空 URL、追加后缀，并将所属来源全部标为失效。
            // 同时检查这些条件，避免仅凭名字误判正常的文本订阅。
            let legacy: Vec<(String, String)> = sqlx::query_as(
                "SELECT id,name FROM subscriptions AS sub WHERE url='' AND name LIKE '%（已移除）' \
                 AND EXISTS(SELECT 1 FROM sources WHERE subscription_id=sub.id) \
                 AND NOT EXISTS(SELECT 1 FROM sources WHERE subscription_id=sub.id AND present<>0)",
            )
            .fetch_all(&mut *tx)
            .await?;
            for (id, name) in legacy {
                let original = name.trim_end_matches("（已移除）").trim();
                sqlx::query("UPDATE subscriptions SET name=?,removed_at=?,error='' WHERE id=?")
                    .bind(if original.is_empty() { &name } else { original })
                    .bind(now())
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("PRAGMA user_version=2")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        if schema < 3 {
            let mut tx = db.begin().await?;
            sqlx::raw_sql(include_str!("../migrations/0003.sql"))
                .execute(&mut *tx)
                .await?;
            sqlx::query("PRAGMA user_version=3")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        if schema < 4 {
            let mut tx = db.begin().await?;
            sqlx::raw_sql(include_str!("../migrations/0004.sql"))
                .execute(&mut *tx)
                .await?;
            sqlx::query("PRAGMA user_version=4")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        if schema < 5 {
            let mut tx = db.begin().await?;
            sqlx::raw_sql(include_str!("../migrations/0005.sql"))
                .execute(&mut *tx)
                .await?;
            sqlx::query("PRAGMA user_version=5")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        if schema < 6 {
            let mut tx = db.begin().await?;
            sqlx::raw_sql(include_str!("../migrations/0006.sql"))
                .execute(&mut *tx)
                .await?;
            sqlx::query("PRAGMA user_version=6")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        let blocked: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key='forbid_proxy'")
                .fetch_one(&db)
                .await?;
        let (policy, _) = watch::channel(blocked == "true");
        Ok(Self(Arc::new(Inner {
            db,
            data_dir: dir,
            forbid_proxy: policy,
            access_changes: watch::channel(0).0,
            allow_private,
            public_url,
            probe_slots: Semaphore::new(2),
            direct_ready_slots: Semaphore::new(4),
            direct_range_slots: Semaphore::new(32),
            direct_range_jobs: Mutex::new(HashMap::new()),
            direct_ready_jobs: Mutex::new(HashMap::new()),
            upstream_clients: crate::net::Clients::default(),
            source_quality_slots: Semaphore::new(1),
            probing: Mutex::new(std::collections::HashSet::new()),
            recommendation_slots: Semaphore::new(4),
            login_budget: Mutex::new(HashMap::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
            native_switch_lock: tokio::sync::Mutex::new(()),
        })))
    }
    pub async fn cache_get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let v: Option<String> =
            sqlx::query_scalar("SELECT value FROM cache WHERE key=? AND expires_at>?")
                .bind(key)
                .bind(now())
                .fetch_optional(&self.db)
                .await?;
        Ok(v.and_then(|s| serde_json::from_str(&s).ok()))
    }
    pub async fn cache_put(&self, key: &str, v: &serde_json::Value, ttl: i64) -> Result<()> {
        sqlx::query("INSERT INTO cache VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,expires_at=excluded.expires_at")
            .bind(key).bind(v.to_string()).bind(now()+ttl).execute(&self.db).await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub admin: bool,
    pub disabled: bool,
}
#[derive(Clone)]
pub struct Identity {
    pub user: User,
    pub auth_hash: String,
    pub client: String,
}
impl Identity {
    pub fn admin(&self) -> Result<()> {
        if self.user.admin {
            Ok(())
        } else {
            Err(Error::denied("仅管理员可执行此操作"))
        }
    }
}
