use crate::core::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
pub enum DoubanImageSource {
    #[default]
    #[serde(rename = "cmliussss-cdn-tencent")]
    Tencent,
    #[serde(rename = "cmliussss-cdn-ali")]
    Ali,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub douban_image_source: DoubanImageSource,
    #[serde(default)]
    pub douban_image_custom_url: String,
}

pub async fn get(app: &App, user: &str) -> Result<Preferences> {
    let saved: Option<String> = sqlx::query_scalar(
        "SELECT value FROM preferences WHERE user_id=? AND key='douban_image_settings'",
    )
    .bind(user)
    .fetch_optional(&app.db)
    .await?;
    Ok(saved
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

pub async fn save(app: &App, user: &str, value: &serde_json::Value) -> Result<Preferences> {
    let mut preferences: Preferences =
        serde_json::from_value(value.clone()).map_err(|_| Error::bad("豆瓣图片源设置无效"))?;
    preferences.douban_image_custom_url = preferences.douban_image_custom_url.trim().to_owned();
    let custom = &preferences.douban_image_custom_url;
    if custom.len() > 2048 {
        return Err(Error::bad("自定义图片源地址过长"));
    }
    if !custom.is_empty() {
        let url = url::Url::parse(custom).map_err(|_| Error::bad("自定义图片源地址无效"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.host_str().is_none()
            || url.fragment().is_some()
        {
            return Err(Error::bad(
                "请填写不含账号、密码或片段的 HTTP(S) 图片源前缀",
            ));
        }
    } else if matches!(preferences.douban_image_source, DoubanImageSource::Custom) {
        return Err(Error::bad("请填写自定义图片源前缀"));
    }
    sqlx::query("INSERT INTO preferences VALUES(?,'douban_image_settings',?) ON CONFLICT(user_id,key) DO UPDATE SET value=excluded.value")
        .bind(user).bind(serde_json::to_string(&preferences).unwrap()).execute(&app.db).await?;
    Ok(preferences)
}
