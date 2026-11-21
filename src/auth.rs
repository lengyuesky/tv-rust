use crate::core::*;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::http::HeaderMap;
use serde_json::{Value, json};

pub fn credentials(username: &str, password: &str) -> Result<()> {
    if !(2..=32).contains(&username.chars().count())
        || !username
            .chars()
            .all(|c| c.is_alphanumeric() || "_- .".contains(c))
    {
        return Err(Error::bad("用户名需为 2–32 个字母、汉字、数字或常用分隔符"));
    }
    if !(8..=128).contains(&password.len()) || password.contains('\0') {
        return Err(Error::bad("密码需为 8–128 字节"));
    }
    Ok(())
}
pub async fn verifier(password: &str) -> Result<String> {
    let digest = hash(password);
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(digest.as_bytes())
            .map(|v| v.to_string())
    })
    .await
    .map_err(|_| Error::bad("密码处理失败"))?
    .map_err(|_| Error::bad("密码处理失败"))
}
pub async fn verify(stored: String, candidate_digest: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored).ok().is_some_and(|p| {
            Argon2::default()
                .verify_password(candidate_digest.as_bytes(), &p)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}
pub fn token(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() < 256)
    {
        return Some(v.strip_prefix("Bearer ").unwrap_or(v).to_owned());
    }
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() < 8192)
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|p| {
                    p.strip_prefix("tv_session=")
                        .or_else(|| p.strip_prefix("Trim-MC-token="))
                })
                .map(str::to_owned)
        })
}
pub async fn identity(app: &App, headers: &HeaderMap) -> Result<Identity> {
    let raw = token(headers).ok_or_else(Error::unauthorized)?;
    let digest = hash(raw);
    identity_for_hash(app, &digest).await
}
pub(crate) async fn identity_for_hash(app: &App, digest: &str) -> Result<Identity> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT user_id,client FROM auth_sessions WHERE token_hash=? AND expires_at>?",
    )
    .bind(digest)
    .bind(now())
    .fetch_optional(&app.db)
    .await?;
    let (id, client) = row.ok_or_else(Error::unauthorized)?;
    let user = sqlx::query_as::<_, User>(
        "SELECT id,username,admin,disabled FROM users WHERE id=? AND disabled=0",
    )
    .bind(&id)
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::unauthorized)?;
    Ok(Identity {
        user,
        auth_hash: digest.to_owned(),
        client,
    })
}
pub async fn setup(app: &App, value: &Value) -> Result<()> {
    let user = text(value, "username").trim().to_owned();
    let password = text(value, "password");
    credentials(&user, &password)?;
    let v = verifier(&password).await?;
    let result = sqlx::query("INSERT INTO users(id,username,verifier,admin,created_at) SELECT ?,?,?,1,? WHERE NOT EXISTS(SELECT 1 FROM users)")
        .bind(guid(&[&random()])).bind(&user).bind(v).bind(now()).execute(&app.db).await?;
    if result.rows_affected() != 1 {
        return Err(Error::denied("首次设置已完成"));
    }
    Ok(())
}
pub async fn login(
    app: &App,
    username: &str,
    password: &str,
    native: bool,
) -> Result<(String, User)> {
    let username = username.trim();
    let key = hash(username.to_lowercase());
    {
        let mut budget = app.login_budget.lock().unwrap();
        budget.retain(|_, (at, _)| *at > now() - 300);
        if budget.len() > 2048 {
            return Err(Error::bad("登录请求过多，请稍后重试"));
        }
        let entry = budget.entry(key.clone()).or_insert((now(), 0));
        entry.1 += 1;
        if entry.1 > 15 {
            return Err(Error(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "登录尝试过多，请五分钟后再试".into(),
            ));
        }
    }
    if username.len() > 128 || password.len() > 128 {
        return Err(Error::unauthorized());
    }
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT id,verifier FROM users WHERE username=? COLLATE NOCASE AND disabled=0",
    )
    .bind(username)
    .fetch_optional(&app.db)
    .await?;
    let (id, stored) = row.ok_or_else(|| Error::denied("用户名或密码错误"))?;
    let digest = if native {
        if password.len() != 64 || !password.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::denied("用户名或密码错误"));
        }
        password.to_ascii_lowercase()
    } else {
        hash(password)
    };
    if !verify(stored, digest).await {
        return Err(Error::denied("用户名或密码错误"));
    }
    app.login_budget.lock().unwrap().remove(&key);
    let user = sqlx::query_as::<_, User>("SELECT id,username,admin,disabled FROM users WHERE id=?")
        .bind(&id)
        .fetch_one(&app.db)
        .await?;
    let raw = random();
    sqlx::query("INSERT INTO auth_sessions VALUES(?,?,?,?)")
        .bind(hash(&raw))
        .bind(&id)
        .bind(if native { "fntv" } else { "web" })
        .bind(now() + 30 * 86400)
        .execute(&app.db)
        .await?;
    Ok((raw, user))
}
pub async fn users(app: &App, who: &Identity) -> Result<Value> {
    who.admin()?;
    let list = sqlx::query_as::<_, User>(
        "SELECT id,username,admin,disabled FROM users ORDER BY created_at",
    )
    .fetch_all(&app.db)
    .await?;
    Ok(json!(list))
}
pub async fn create_user(app: &App, who: &Identity, v: &Value) -> Result<()> {
    who.admin()?;
    let name = text(v, "username").trim().to_owned();
    let pass = text(v, "password");
    credentials(&name, &pass)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&app.db)
        .await?;
    if count >= 100 {
        return Err(Error::bad("最多支持 100 个家庭账号"));
    }
    let verifier = verifier(&pass).await?;
    sqlx::query("INSERT INTO users(id,username,verifier,admin,created_at) VALUES(?,?,?,?,?)")
        .bind(guid(&[&random()]))
        .bind(name)
        .bind(verifier)
        .bind(v["admin"].as_bool().unwrap_or(false))
        .bind(now())
        .execute(&app.db)
        .await
        .map_err(|_| Error::bad("创建失败，用户名可能已存在"))?;
    Ok(())
}
pub async fn change_password(app: &App, who: &Identity, v: &Value) -> Result<()> {
    let old = text(v, "current_password");
    let pass = text(v, "password");
    credentials(&who.user.username, &pass)?;
    let stored: String = sqlx::query_scalar("SELECT verifier FROM users WHERE id=?")
        .bind(&who.user.id)
        .fetch_one(&app.db)
        .await?;
    if !verify(stored, hash(old)).await {
        return Err(Error::denied("当前密码错误"));
    }
    reset_password(app, &who.user.username, &pass).await
}
pub async fn reset_password(app: &App, username: &str, password: &str) -> Result<()> {
    credentials(username, password)?;
    let v = verifier(password).await?;
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM users WHERE username=? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&mut *tx)
            .await?;
    let id = id.ok_or_else(Error::missing)?;
    sqlx::query("UPDATE users SET verifier=? WHERE id=?")
        .bind(v)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM auth_sessions WHERE user_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE plays SET ended=1 WHERE user_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
