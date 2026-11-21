//! 飞牛的作品、季和单集身份独立于实际视频源；网页及来源进度仍保留原身份。
use crate::{access, catalog, core::*};
use catalog::{Episode, Film};
use serde_json::Value;

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct Work {
    pub id: String,
    pub group_key: String,
    pub season_id: String,
}

pub(crate) fn key(film: &Film) -> String {
    if catalog::known_year(film.year.trim()) {
        catalog::group_key(film)
    } else {
        film.id.clone()
    }
}

pub(crate) async fn work(app: &App, film: &Film) -> Result<Work> {
    let group_key = key(film);
    let id = guid(&["fntv-work", &group_key]);
    let season_id = crate::fntv::season(&id);
    sqlx::query("INSERT OR IGNORE INTO fntv_works VALUES(?,?,?)")
        .bind(&id)
        .bind(&group_key)
        .bind(&season_id)
        .execute(&app.db)
        .await?;
    Ok(Work {
        id,
        group_key,
        season_id,
    })
}

pub(crate) async fn find(app: &App, id: &str) -> Result<Option<Work>> {
    Ok(
        sqlx::query_as("SELECT * FROM fntv_works WHERE id=? OR season_id=?")
            .bind(id)
            .bind(id)
            .fetch_optional(&app.db)
            .await?,
    )
}

pub(crate) async fn members(app: &App, key: &str) -> Result<Vec<Film>> {
    let group = serde_json::from_str::<Value>(key).ok();
    let films: Vec<Film> = if let Some(group) = group.filter(Value::is_array) {
        sqlx::query_as("SELECT * FROM films WHERE lower(trim(title))=? AND trim(year)=? AND kind=? ORDER BY id")
            .bind(group[0].as_str().unwrap_or_default())
            .bind(group[1].as_str().unwrap_or_default())
            .bind(group[2].as_str().unwrap_or_default())
            .fetch_all(&app.db).await?
    } else {
        sqlx::query_as("SELECT * FROM films WHERE id=?")
            .bind(key)
            .fetch_all(&app.db)
            .await?
    };
    Ok(films
        .into_iter()
        .filter(|film| self::key(film) == key)
        .collect())
}

pub(crate) async fn preferred(app: &App, who: &Identity, work: &Work) -> Result<Film> {
    let films = members(app, &work.group_key).await?;
    let active: Vec<String> = sqlx::query_scalar("SELECT film_id FROM plays WHERE auth_hash=? AND ended=0 AND expires_at>? ORDER BY seq DESC")
        .bind(&who.auth_hash).bind(now()).fetch_all(&app.db).await?;
    let latest: Vec<String> = sqlx::query_scalar("SELECT film_id FROM progress WHERE user_id=? ORDER BY updated_at DESC,authority DESC,film_id")
        .bind(&who.user.id).fetch_all(&app.db).await?;
    let mut films = films;
    films.sort_by_key(|film| {
        (
            active
                .iter()
                .position(|id| id == &film.id)
                .unwrap_or(usize::MAX),
            latest
                .iter()
                .position(|id| id == &film.id)
                .unwrap_or(usize::MAX),
        )
    });
    for film in &films {
        if access::source_allowed(app, &who.user.id, &film.source_id).await?
            && catalog::source(app, &film.source_id).await?.active()
        {
            return Ok(film.clone());
        }
    }
    for film in films {
        if access::source_allowed(app, &who.user.id, &film.source_id).await? {
            return Ok(film);
        }
    }
    Err(Error::denied(access::UNAUTHORIZED_SOURCE))
}

pub(crate) async fn episode_guid(app: &App, film: &Film, episode: &Episode) -> Result<String> {
    let Some(number) = catalog::episode_number(&episode.title) else {
        return Ok(episode.id.clone());
    };
    let duplicates = catalog::episodes(app, &film.id)
        .await?
        .into_iter()
        .filter(|e| {
            e.line_name == episode.line_name && catalog::episode_number(&e.title) == Some(number)
        })
        .count();
    if duplicates != 1 {
        return Ok(episode.id.clone());
    }
    let work = work(app, film).await?;
    let id = guid(&["fntv-work-episode", &work.id, &number.to_string()]);
    sqlx::query("INSERT OR IGNORE INTO fntv_work_episodes VALUES(?,?,?)")
        .bind(&id)
        .bind(work.id)
        .bind(i64::from(number))
        .execute(&app.db)
        .await?;
    Ok(id)
}

pub(crate) async fn episode_work(app: &App, id: &str) -> Result<Option<(Work, u32)>> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT work_id,episode_number FROM fntv_work_episodes WHERE id=?")
            .bind(id)
            .fetch_optional(&app.db)
            .await?;
    match row {
        Some((work, number)) => Ok(Some((
            find(app, &work).await?.ok_or_else(Error::missing)?,
            number as u32,
        ))),
        None => Ok(None),
    }
}

pub(crate) async fn matches_episode(
    app: &App,
    item: &str,
    episode: &Episode,
) -> Result<Option<bool>> {
    let film = catalog::film(app, &episode.film_id).await?;
    if let Some((work, number)) = episode_work(app, item).await? {
        return Ok(Some(
            work.group_key == key(&film) && catalog::episode_number(&episode.title) == Some(number),
        ));
    }
    if let Some(work) = find(app, item).await? {
        return Ok(Some(work.group_key == key(&film)));
    }
    Ok(None)
}
