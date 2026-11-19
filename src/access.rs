use crate::{catalog::Source, core::*};
use serde::{Deserialize, Serialize};

pub const UNAUTHORIZED_SOURCE: &str = "当前账号未获使用权限";

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UsageMode {
    #[default]
    All,
    Selected,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub mode: UsageMode,
    #[serde(default)]
    pub user_ids: Vec<String>,
}

impl Usage {
    pub fn parse(value: &serde_json::Value) -> Result<Self> {
        let usage: Self =
            serde_json::from_value(value.clone()).map_err(|_| Error::bad("订阅使用范围无效"))?;
        if usage.user_ids.len() > 100
            || usage
                .user_ids
                .iter()
                .any(|id| id.is_empty() || id.len() > 128)
            || (usage.mode == UsageMode::Selected && usage.user_ids.is_empty())
            || (usage.mode == UsageMode::All && !usage.user_ids.is_empty())
        {
            return Err(Error::bad("指定用户至少选择一位成员，所有用户模式无需名单"));
        }
        Ok(usage)
    }
}

pub async fn source_allowed(app: &App, user: &str, source: &str) -> Result<bool> {
    let mut connection = app.db.acquire().await?;
    source_allowed_on(&mut connection, user, source).await
}

pub async fn source_allowed_on(
    connection: &mut sqlx::SqliteConnection,
    user: &str,
    source: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sources s JOIN subscriptions sub ON sub.id=s.subscription_id \
         JOIN users u ON u.id=? WHERE s.id=? AND u.disabled=0 AND \
         (u.admin=1 OR sub.all_users=1 OR EXISTS(SELECT 1 FROM subscription_users su \
         WHERE su.subscription_id=sub.id AND su.user_id=u.id)))",
    )
    .bind(user)
    .bind(source)
    .fetch_one(connection)
    .await?)
}

pub async fn require_source(app: &App, user: &str, source: &str) -> Result<()> {
    if source_allowed(app, user, source).await? {
        Ok(())
    } else {
        Err(Error::denied(UNAUTHORIZED_SOURCE))
    }
}

pub async fn sources(app: &App, user: &str) -> Result<Vec<Source>> {
    Ok(sqlx::query_as(
        "SELECT s.* FROM sources s JOIN subscriptions sub ON sub.id=s.subscription_id \
         JOIN users u ON u.id=? WHERE u.disabled=0 AND sub.removed_at=0 AND \
         (u.admin=1 OR sub.all_users=1 OR EXISTS(SELECT 1 FROM subscription_users su \
         WHERE su.subscription_id=sub.id AND su.user_id=u.id)) ORDER BY s.name,s.id",
    )
    .bind(user)
    .fetch_all(&app.db)
    .await?)
}

pub async fn write_usage(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
    usage: &Usage,
) -> Result<()> {
    for user in &usage.user_ids {
        let member: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=? AND admin=0)")
                .bind(user)
                .fetch_one(&mut *connection)
                .await?;
        if !member {
            return Err(Error::bad("请选择存在的普通成员；管理员始终可用"));
        }
    }
    let updated = sqlx::query("UPDATE subscriptions SET all_users=? WHERE id=? AND removed_at=0")
        .bind(usage.mode == UsageMode::All)
        .bind(id)
        .execute(&mut *connection)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(Error::missing());
    }
    sqlx::query("DELETE FROM subscription_users WHERE subscription_id=?")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    for user in &usage.user_ids {
        sqlx::query("INSERT OR IGNORE INTO subscription_users VALUES(?,?)")
            .bind(id)
            .bind(user)
            .execute(&mut *connection)
            .await?;
    }
    // 撤销使用范围与播放令牌处于同一事务；个人进度保持最后已保存值。
    let revoked: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.id,p.mode FROM plays p JOIN films f ON f.id=p.film_id \
         JOIN sources s ON s.id=f.source_id JOIN subscriptions sub ON sub.id=s.subscription_id \
         JOIN users u ON u.id=p.user_id WHERE sub.id=? AND p.ended=0 AND \
         (u.disabled=1 OR (u.admin=0 AND sub.all_users=0 AND NOT EXISTS \
         (SELECT 1 FROM subscription_users su WHERE su.subscription_id=sub.id AND su.user_id=u.id)))",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    for (play, mode) in revoked {
        sqlx::query("UPDATE plays SET ended=1,state='access_revoked' WHERE id=?")
            .bind(&play)
            .execute(&mut *connection)
            .await?;
        sqlx::query("INSERT INTO play_events(play_id,at,mode,event,message) VALUES(?,?,?,'access_revoked',?)")
            .bind(play).bind(now()).bind(mode).bind(UNAUTHORIZED_SOURCE)
            .execute(&mut *connection).await?;
    }
    sqlx::query("DELETE FROM cache WHERE key LIKE 'search:%'")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn save_usage(app: &App, id: &str, value: &serde_json::Value) -> Result<Usage> {
    let usage = Usage::parse(value)?;
    let _guard = app.refresh_lock.lock().await;
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    write_usage(&mut tx, id, &usage).await?;
    tx.commit().await?;
    app.access_changes
        .send_modify(|revision| *revision = revision.wrapping_add(1));
    Ok(usage)
}
