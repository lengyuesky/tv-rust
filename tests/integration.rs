use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
use tv_rust::{auth, catalog, core::*, playback, router};

struct Harness {
    _dir: tempfile::TempDir,
    app: App,
    who: Identity,
    other: Identity,
    raw: String,
    film: String,
    episode: String,
    server: tokio::task::JoinHandle<()>,
    hits: Arc<AtomicUsize>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn harness() -> Harness {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream=Router::new().fallback(move |req:Request<Body>|{let hits=counter.clone();async move{
        hits.fetch_add(1,Ordering::Relaxed);
        match req.uri().path(){
            "/master"=>([("content-type","application/vnd.apple.mpegurl")],"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=10000\nmedia.m3u8?token=secret\n").into_response(),
            "/media.m3u8"=>{
                if req.headers().contains_key("range") {Response::builder().status(206).header("content-range","bytes 0-0/160").body(Body::from("#")).unwrap()}
                else {([("content-type","application/vnd.apple.mpegurl")],"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin?secret=1\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nsegment.ts\n#EXT-X-ENDLIST\n").into_response()}
            },
            "/large.m3u8"=>{
                let mut body=String::from("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin?secret=1\"\n#EXT-X-MAP:URI=\"init.mp4\"\n");
                for n in 0..2400{body.push_str(&format!("#EXTINF:4,\nsegment-{n}.ts?token=secret\n"));}
                body.push_str("#EXT-X-ENDLIST\n");
                ([("content-type","application/vnd.apple.mpegurl")],body).into_response()
            },
            "/encrypted.m3u8"=>([("content-type","application/vnd.apple.mpegurl")],include_str!("fixtures/hls-probe/media.m3u8")).into_response(),
            "/key.bin"=>include_bytes!("fixtures/hls-probe/key.bin").as_slice().into_response(),
            "/segment-0.ts"=>include_bytes!("fixtures/hls-probe/segment-0.ts").as_slice().into_response(),
            "/slow"=>Response::builder().header("content-type","video/mp4").body(Body::from_stream(async_stream::stream!{
                yield Ok::<_,std::io::Error>(bytes::Bytes::from_static(b"first-chunk"));
                tokio::time::sleep(Duration::from_secs(20)).await;
                yield Ok::<_,std::io::Error>(bytes::Bytes::from_static(b"later-chunk"));
            })).unwrap(),
            "/api"=>axum::Json(json!({"list":[{"vod_id":99,"vod_name":"原生搜索结果","vod_year":"2026","vod_play_url":""}],"page":"1","pagecount":1})).into_response(),
            "/noisy-api"=>axum::Json(json!({"list":[
                {"vod_id":991,"vod_name":"11月19日 26-27赛季NBA常规赛","vod_year":"2026"},
                {"vod_id":992,"vod_name":"乔安你好","vod_year":"2019"},
                {"vod_id":993,"vod_name":"你好","vod_year":"2021"},
                {"vod_id":994,"vod_name":"<b>你好</b>，世界","vod_year":"2023"},
                {"vod_id":993,"vod_name":"你好","vod_year":"2021"}
            ],"page":"1","pagecount":6303})).into_response(),
            "/config"=>axum::Json(json!({"api_site":{"same.key":{"name":"更新源","api":"https://example.com/api"}}})).into_response(),
            _=>{
                if req.headers().get("range").is_some(){Response::builder().status(206).header("content-range","bytes 2-5/8").header("accept-ranges","bytes").body(Body::from("2345")).unwrap()}
                else{Response::builder().header("content-type","video/mp4").body(Body::from("01234567")).unwrap()}
            }
        }
    }});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(dir.path().into(), true, String::new())
        .await
        .unwrap();
    auth::setup(&app, &json!({"username":"admin","password":"password-123"}))
        .await
        .unwrap();
    let (raw, user) = auth::login(&app, "admin", "password-123", false)
        .await
        .unwrap();
    let who = Identity {
        user,
        auth_hash: hash(&raw),
        client: "web".into(),
    };
    auth::create_user(
        &app,
        &who,
        &json!({"username":"member","password":"password-456"}),
    )
    .await
    .unwrap();
    let (other_raw, user) = auth::login(&app, "member", "password-456", false)
        .await
        .unwrap();
    let other = Identity {
        user,
        auth_hash: hash(other_raw),
        client: "web".into(),
    };
    let config = json!({"cache_time":7200,"api_site":{"same.key":{"name":"测试源","api":format!("{base}/api"),"_comment":"说明"}}});
    catalog::import(&app, &json!({"name":"订阅一","content":config.to_string()}))
        .await
        .unwrap();
    let source = catalog::sources(&app).await.unwrap().remove(0);
    let f=catalog::ingest(&app,&source,&json!({"vod_id":1,"vod_name":"测试电影","vod_year":"2026","vod_play_from":"line","vod_play_url":format!("正片${base}/video.mp4")}),true).await.unwrap().unwrap();
    let ep = catalog::episodes(&app, &f.id).await.unwrap().remove(0);
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(json!({"kind":"mp4","duration":120.0,"size":8,"streams":[],"warning":""}).to_string())
        .bind(now())
        .bind(&ep.id)
        .execute(&app.db)
        .await
        .unwrap();
    Harness {
        _dir: dir,
        app,
        who,
        other,
        raw,
        film: f.id,
        episode: ep.id,
        server,
        hits,
    }
}
async fn request(
    h: &Harness,
    method: &str,
    path: &str,
    value: Value,
    raw: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("x-tv-request", "1")
        .header("content-type", "application/json")
        .header("host", "127.0.0.1:3000");
    if let Some(raw) = raw {
        builder = builder.header("authorization", format!("Bearer {raw}"));
    }
    let r = router(h.app.clone(), "/nonexistent")
        .oneshot(builder.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn removing_a_subscription_is_idempotent_and_preserves_personal_records() {
    let h = harness().await;
    let original = catalog::sources(&h.app).await.unwrap().remove(0);
    let second = catalog::import(&h.app, &json!({"name":"其他订阅","content":json!({"api_site":{"other":{"name":"其他源","api":original.api}}}).to_string()})).await.unwrap();
    catalog::favorite(&h.app, &h.who.user.id, &h.film, true)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let play = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    playback::record(&h.app, &play, 42.0, 120.0, None)
        .await
        .unwrap();
    let (member, _) = auth::login(&h.app, "member", "password-456", false)
        .await
        .unwrap();
    let path = format!("/api/subscriptions/{}", original.subscription_id);
    assert_eq!(
        request(&h, "DELETE", &path, json!({}), Some(&member))
            .await
            .0,
        403
    );
    assert_eq!(request(&h, "DELETE", &path, json!({}), None).await.0, 401);
    assert_eq!(
        request(&h, "DELETE", &path, json!({}), Some(&h.raw))
            .await
            .0,
        200
    );
    let removed: (String, i64) =
        sqlx::query_as("SELECT name,removed_at FROM subscriptions WHERE id=?")
            .bind(&original.subscription_id)
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    assert_eq!(removed.0, "订阅一");
    assert!(removed.1 > 0);
    assert_eq!(
        request(&h, "DELETE", &path, json!({}), Some(&h.raw))
            .await
            .0,
        200
    );
    let repeated: (String, i64) =
        sqlx::query_as("SELECT name,removed_at FROM subscriptions WHERE id=?")
            .bind(&original.subscription_id)
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    assert_eq!(repeated, removed);
    assert_eq!(
        request(
            &h,
            "DELETE",
            "/api/subscriptions/missing",
            json!({}),
            Some(&h.raw)
        )
        .await
        .0,
        404
    );
    let listing = catalog::subscription_list(&h.app).await.unwrap();
    assert_eq!(listing["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(listing["subscriptions"][0]["id"], second);
    assert_eq!(listing["sources"].as_array().unwrap().len(), 1);
    assert!(
        !catalog::source(&h.app, &original.id)
            .await
            .unwrap()
            .active()
    );
    for kind in ["favorites", "history"] {
        let items = catalog::personal(&h.app, &h.who.user.id, kind)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["available"], false);
        assert_eq!(items[0]["progress"]["position"], 42.0);
    }
    assert!(
        !playback::logs(&h.app, &h.who, 0).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(playback::start(&h.app, &h.who, &h.episode).await.is_err());
    assert!(
        catalog::refresh(&h.app, &original.subscription_id)
            .await
            .is_err()
    );
    let config =
        json!({"api_site":{"same.key":{"name":"重新导入","api":original.api}}}).to_string();
    assert!(
        catalog::import(
            &h.app,
            &json!({"id":original.subscription_id,"name":"订阅一","content":config})
        )
        .await
        .is_err()
    );
    let imported = catalog::import(&h.app, &json!({"name":"订阅一","content":config}))
        .await
        .unwrap();
    assert_ne!(imported, original.subscription_id);
    assert!(!catalog::source(&h.app, &original.id).await.unwrap().present);
}

#[tokio::test]
async fn subscription_removal_rolls_back_and_cannot_be_undone_by_refresh() {
    let h = harness().await;
    let original = catalog::sources(&h.app).await.unwrap().remove(0);
    sqlx::raw_sql("CREATE TRIGGER reject_removal BEFORE UPDATE OF present ON sources WHEN NEW.present=0 BEGIN SELECT RAISE(ABORT,'controlled failure'); END;")
        .execute(&h.app.db).await.unwrap();
    assert!(
        catalog::remove_subscription(&h.app, &original.subscription_id)
            .await
            .is_err()
    );
    let removed: i64 = sqlx::query_scalar("SELECT removed_at FROM subscriptions WHERE id=?")
        .bind(&original.subscription_id)
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(removed, 0);
    assert!(
        catalog::source(&h.app, &original.id)
            .await
            .unwrap()
            .active()
    );
    sqlx::query("DROP TRIGGER reject_removal")
        .execute(&h.app.db)
        .await
        .unwrap();
    sqlx::query("UPDATE subscriptions SET url=? WHERE id=?")
        .bind(original.api.replace("/api", "/config"))
        .bind(&original.subscription_id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let (refresh, removal) = tokio::join!(
        catalog::refresh(&h.app, &original.subscription_id),
        catalog::remove_subscription(&h.app, &original.subscription_id),
    );
    assert!(refresh.is_ok());
    assert!(removal.is_ok());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT url FROM subscriptions WHERE id=?")
            .bind(&original.subscription_id)
            .fetch_one(&h.app.db)
            .await
            .unwrap(),
        ""
    );
    assert!(
        !catalog::source(&h.app, &original.id)
            .await
            .unwrap()
            .active()
    );
    let hits = h.hits.load(Ordering::Relaxed);
    assert!(
        catalog::refresh(&h.app, &original.subscription_id)
            .await
            .is_err()
    );
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
}

#[tokio::test]
async fn legacy_removals_are_migrated_once_without_losing_favorites() {
    let dir = tempfile::tempdir().unwrap();
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(dir.path().join("tv.sqlite"))
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0001.sql"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO subscriptions VALUES('old','lunatv（已移除）（已移除）（已移除）','','{}',7200,1,1,'旧错误');
        INSERT INTO subscriptions VALUES('text','正常（已移除）','','{}',7200,1,1,'');
        INSERT INTO subscriptions VALUES('empty','空配置（已移除）','','{}',7200,1,1,'');
        INSERT INTO sources(id,subscription_id,source_key,name,api,present) VALUES('old-source','old','key','旧源','https://example.com/api',0);
        INSERT INTO sources(id,subscription_id,source_key,name,api,present) VALUES('text-source','text','key','正常文本源','https://example.com/api',1);
        INSERT INTO users VALUES('user','admin','unused',1,0,1);
        INSERT INTO films(id,source_id,vod_id,title,updated_at) VALUES('film','old-source','1','旧收藏',1);
        INSERT INTO favorites VALUES('user','film',1);")
        .execute(&db).await.unwrap();
    db.close().await;
    let app = App::new(dir.path().into(), true, String::new())
        .await
        .unwrap();
    let legacy: (String, i64, String) =
        sqlx::query_as("SELECT name,removed_at,error FROM subscriptions WHERE id='old'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(legacy.0, "lunatv");
    assert!(legacy.1 > 0);
    assert!(legacy.2.is_empty());
    let listing = catalog::subscription_list(&app).await.unwrap();
    assert_eq!(listing["subscriptions"].as_array().unwrap().len(), 2);
    assert_eq!(listing["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        catalog::personal(&app, "user", "favorites").await.unwrap()[0]["available"],
        false
    );
    app.db.close().await;
    let again = App::new(dir.path().into(), true, String::new())
        .await
        .unwrap();
    let repeated: (String, i64, String) =
        sqlx::query_as("SELECT name,removed_at,error FROM subscriptions WHERE id='old'")
            .fetch_one(&again.db)
            .await
            .unwrap();
    assert_eq!(repeated, legacy);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&again.db)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn search_filters_unrelated_rows_before_ingest_and_ranks_titles_for_both_clients() {
    let h = harness().await;
    let original = catalog::sources(&h.app).await.unwrap().remove(0);
    let config = json!({"api_site":{"same.key":{"name":"测试源","api":original.api.replace("/api", "/noisy-api")}}}).to_string();
    catalog::import(
        &h.app,
        &json!({"id":original.subscription_id,"name":"订阅一","content":config}),
    )
    .await
    .unwrap();
    let result = catalog::search(&h.app, "你好", 1, None).await.unwrap();
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["title"], "你好");
    assert_eq!(items[0]["search_rank"], 0);
    assert_eq!(result["sources"][0]["filtered_count"], 1);
    assert!(items.iter().all(|v| !text(v, "title").contains("NBA")));
    let noise = guid(&["film", &original.id, "991"]);
    assert!(catalog::film(&h.app, &noise).await.is_err());
    let hits = h.hits.load(Ordering::Relaxed);
    assert_eq!(
        catalog::search(&h.app, "你好", 1, None).await.unwrap(),
        result
    );
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
    let nba = catalog::search(&h.app, "ＮＢＡ", 1, None).await.unwrap();
    assert_eq!(nba["items"].as_array().unwrap().len(), 1);
    assert!(text(&nba["items"][0], "title").contains("NBA"));
    let words = catalog::search(&h.app, "你好 乔安", 1, None).await.unwrap();
    assert_eq!(words["items"].as_array().unwrap().len(), 1);
    assert_eq!(words["items"][0]["search_rank"], 2);
    let empty = catalog::search(&h.app, "完全不相关", 1, None)
        .await
        .unwrap();
    assert!(empty["items"].as_array().unwrap().is_empty());
    assert_eq!(empty["has_more"], true);
    assert_eq!(
        catalog::search(&h.app, "完全不相关", 500, None)
            .await
            .unwrap()["has_more"],
        false
    );
    assert!(catalog::search(&h.app, "!!!", 1, None).await.is_err());
    let (status, native) = request(
        &h,
        "POST",
        "/v/api/v1/item/list",
        json!({"q":"你好","page":1,"page_size":50}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(native["data"]["list"][0]["title"], "你好 · 测试源");
    assert!(
        native["data"]["list"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| !text(v, "title").contains("NBA"))
    );
}

#[tokio::test]
async fn poster_urls_are_resolved_and_backups_only_use_the_same_known_work() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let base = source.api.strip_suffix("/api").unwrap();
    let primary = catalog::ingest(&h.app, &source, &json!({"vod_id":1,"vod_name":"测试电影","vod_year":"2026","vod_pic":"/posters/original.jpg"}), false).await.unwrap().unwrap();
    assert_eq!(primary.poster, format!("{base}/posters/original.jpg"));
    let mut wrapped = source.clone();
    wrapped.api = "https://wrapper.example/?url=https%3A%2F%2Forigin.example%2Fapi%2Fvod".into();
    let resolved = catalog::ingest(
        &h.app,
        &wrapped,
        &json!({"vod_id":8,"vod_name":"封装接口图片","vod_pic":"/covers/a.jpg"}),
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.poster, "https://origin.example/covers/a.jpg");
    for (id, year, category, remarks, path) in [
        (2, "2026", "电影", "", "//images.example/backup.jpg"),
        (3, "2025", "电影", "", "/wrong-year.jpg"),
        (4, "", "电影", "", "/unknown-year.jpg"),
        (5, "2026", "电视剧", "", "/wrong-kind.jpg"),
        (6, "2026", "电影", "导演剪辑", "/wrong-version.jpg"),
    ] {
        catalog::ingest(&h.app, &source, &json!({"vod_id":id,"vod_name":"测试电影","vod_year":year,"type_name":category,"vod_remarks":remarks,"vod_pic":path}), false).await.unwrap();
    }
    let unsafe_url = catalog::ingest(
        &h.app,
        &source,
        &json!({"vod_id":7,"vod_name":"无效图片","vod_pic":"javascript:alert(1)"}),
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(unsafe_url.poster.is_empty());
    let hits = h.hits.load(Ordering::Relaxed);
    let candidates = catalog::film_posters(&h.app, &primary.id).await.unwrap();
    let urls = candidates["urls"].as_array().unwrap();
    assert_eq!(urls.len(), 2);
    assert!(urls.contains(&json!(primary.poster)));
    assert!(urls.contains(&json!("http://images.example/backup.jpg")));
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
    app_seed_recommendation(&h.app, "123", "测试电影").await;
    h.app
        .cache_put(
            "recommendation-meta:v1:123",
            &json!({"year":"2026","kind":"Movie"}),
            3600,
        )
        .await
        .unwrap();
    let (status, rec) = request(
        &h,
        "GET",
        "/api/recommendations/123/posters?kind=movie",
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(rec, candidates);
    assert_eq!(
        request(
            &h,
            "GET",
            "/api/recommendations/123/posters",
            json!({}),
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(
        request(
            &h,
            "GET",
            "/api/recommendations/456/posters",
            json!({}),
            Some(&h.raw)
        )
        .await
        .0,
        404
    );
    catalog::remove_subscription(&h.app, &source.subscription_id)
        .await
        .unwrap();
    assert_eq!(
        catalog::film_posters(&h.app, &primary.id).await.unwrap(),
        json!({"urls":[]})
    );
}

async fn app_seed_recommendation(app: &App, id: &str, title: &str) {
    app.cache_put("recommend:v2:movie", &json!({"items":[{"id":id,"title":title,"recommendation_kind":"movie","cover":"https://images.example/original.jpg"}]}), 3600).await.unwrap();
}

#[test]
fn config_accepts_json_base58_domains_and_comments() {
    let input=json!({"cache_time":7200,"api_site":{"sample.domain":{"name":"备注不是停用","api":"https://example.com/provide/vod/from/foo/at/json","_comment":"备用"}}}).to_string();
    let direct = catalog::decode(&input).unwrap();
    let encoded = bs58::encode(input.as_bytes()).into_string();
    assert_eq!(catalog::decode(&encoded).unwrap(), direct);
    assert!(catalog::decode("{} ").is_err());
    assert!(
        catalog::decode(r#"{"api_site":{"bad":{"name":"x","api":"file:///etc/passwd"}}}"#).is_err()
    );
}
#[test]
fn api_parameters_preserve_original_paths_and_proxy_wrapper() {
    let url = catalog::api_url(
        "https://example.com/provide/vod/from/a?foo=bar",
        &[("ac", "videolist"), ("wd", "测试")],
    )
    .unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.path(), "/provide/vod/from/a");
    assert!(parsed.query_pairs().any(|(k, v)| k == "wd" && v == "测试"));
    let wrapped = catalog::api_url(
        "https://proxy.example/?url=https%3A%2F%2Fexample.com%2Fapi%3Ftoken%3Dx",
        &[("ac", "detail"), ("ids", "1")],
    )
    .unwrap();
    let outer = url::Url::parse(&wrapped).unwrap();
    let nested = outer
        .query_pairs()
        .find(|(k, _)| k == "url")
        .unwrap()
        .1
        .into_owned();
    let nested = url::Url::parse(&nested).unwrap();
    assert!(nested.query_pairs().any(|(k, v)| k == "ids" && v == "1"));
    assert!(nested.query_pairs().any(|(k, v)| k == "token" && v == "x"));
}
#[test]
fn playlist_keeps_all_lines_and_signed_hls_urls() {
    let list = catalog::playlist(
        &json!({"vod_play_from":"线路一$$$线路二","vod_play_url":"第一集$https://x.example:65/a.m3u8?token=a#第二集$https://x.example/no-extension#$$$正片$https://y.example/a.mp4#"}),
    );
    assert_eq!(list.len(), 3);
    assert!(list[0].4.contains(":65/"));
    assert_eq!(list[1].3, 2);
    assert_eq!(list[2].0, "线路二");
}

#[tokio::test]
async fn imports_are_empty_by_default_and_stable_across_updates() {
    let h = harness().await;
    let initial = catalog::sources(&h.app).await.unwrap()[0].clone();
    assert!(initial.enabled && initial.note == "说明");
    sqlx::query("UPDATE sources SET enabled=0 WHERE id=?")
        .bind(&initial.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    catalog::favorite(&h.app, &h.who.user.id, &h.film, true)
        .await
        .unwrap();
    let config = json!({"api_site":{}}).to_string();
    catalog::import(
        &h.app,
        &json!({"id":initial.subscription_id,"name":"更新","content":config}),
    )
    .await
    .unwrap();
    let gone = catalog::source(&h.app, &initial.id).await.unwrap();
    assert!(!gone.present && !gone.enabled);
    assert_eq!(
        catalog::personal(&h.app, &h.who.user.id, "favorites")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(playback::start(&h.app, &h.who, &h.episode).await.is_err());
    let config = json!({"api_site":{"same.key":{"name":"恢复","api":initial.api}}}).to_string();
    catalog::import(
        &h.app,
        &json!({"id":initial.subscription_id,"name":"更新","content":config}),
    )
    .await
    .unwrap();
    assert!(!catalog::source(&h.app, &initial.id).await.unwrap().enabled);
    catalog::import(&h.app, &json!({"name":"另一订阅","content":config}))
        .await
        .unwrap();
    let all = catalog::sources(&h.app).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_ne!(all[0].id, all[1].id);
}
#[tokio::test]
async fn progress_and_logs_are_private_and_late_reports_do_not_override() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    assert!(
        playback::record(&h.app, &p, 42.0, 120.0, None)
            .await
            .unwrap()
    );
    assert!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        playback::logs(&h.app, &h.other, 0).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(playback::owned(&h.app, &h.other, &p.id).await.is_err());
    let newer = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let newer = playback::get(&h.app, &text(&newer, "id")).await.unwrap();
    assert!(
        playback::record(&h.app, &newer, 9.0, 120.0, None)
            .await
            .unwrap()
    );
    assert!(
        !playback::record(&h.app, &p, 80.0, 120.0, None)
            .await
            .unwrap()
    );
    let v = catalog::view(&h.app, &h.who.user.id, &h.film, false)
        .await
        .unwrap();
    assert_eq!(v["progress"]["position"], 9.0);
    playback::end(&h.app, &newer).await.unwrap();
    let ended = playback::get(&h.app, &newer.id).await.unwrap();
    assert!(
        !playback::record(&h.app, &ended, 85.0, 120.0, None)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn concurrent_progress_and_events_save_without_database_conflicts() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    let gate = Arc::new(tokio::sync::Barrier::new(33));
    let mut workers = tokio::task::JoinSet::new();
    for n in 0..32 {
        let app = h.app.clone();
        let play = p.clone();
        let gate = gate.clone();
        workers.spawn(async move {
            gate.wait().await;
            playback::record(&app, &play, n as f64, 120.0, None).await?;
            playback::event(&app, &play, "playing", "并发播放事件").await
        });
    }
    gate.wait().await;
    while let Some(result) = workers.join_next().await {
        result
            .unwrap()
            .expect("进度与播放事件并发时不应发生写入冲突");
    }
    let saved = playback::get(&h.app, &p.id).await.unwrap();
    assert_eq!(saved.reports, 32);
    let first_reports: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM play_events WHERE play_id=? AND event='progress_reported'",
    )
    .bind(&p.id)
    .fetch_one(&h.app.db)
    .await
    .unwrap();
    assert_eq!(first_reports, 1);
}
#[tokio::test]
async fn native_progress_rejects_a_conflicting_play_link() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    assert!(
        playback::record(&h.app, &p, 12.0, 120.0, Some("current-play"))
            .await
            .unwrap()
    );
    assert!(
        !playback::record(&h.app, &p, 80.0, 120.0, Some("old-play"))
            .await
            .unwrap()
    );
    let view = catalog::view(&h.app, &h.who.user.id, &h.film, false)
        .await
        .unwrap();
    assert_eq!(view["progress"]["position"], 12.0);
}

#[tokio::test]
async fn source_switch_matches_labels_without_copying_personal_progress() {
    let h = harness().await;
    let original_source = catalog::sources(&h.app).await.unwrap().remove(0);
    let config = json!({"api_site":{"same.key":{"name":"另一来源","api":original_source.api}}});
    let subscription = catalog::import(
        &h.app,
        &json!({"name":"第二订阅","content":config.to_string()}),
    )
    .await
    .unwrap();
    let target_source = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.subscription_id == subscription)
        .unwrap();
    let original = catalog::ingest(&h.app, &original_source, &json!({"vod_id":10,"vod_name":"换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"原线路",
        "vod_play_url":"第01集$https://example.com/1.m3u8#第03集$https://example.com/3.m3u8"}), true).await.unwrap().unwrap();
    let target = catalog::ingest(&h.app, &target_source, &json!({"vod_id":20,"vod_name":"换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"新线路",
        "vod_play_url":"第一集$https://example.com/one.m3u8#第二集$https://example.com/two.m3u8#第三集$https://example.com/three.m3u8"}), true).await.unwrap().unwrap();
    let ep = catalog::episodes(&h.app, &original.id)
        .await
        .unwrap()
        .remove(1);
    assert_eq!(ep.number, 2, "源目录中的排序位置不等于第三集的实际集号");
    sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?)")
        .bind(&h.who.user.id)
        .bind(&original.id)
        .bind(&ep.id)
        .bind(36.0)
        .bind(120.0)
        .bind(1_i64)
        .bind(now())
        .execute(&h.app.db)
        .await
        .unwrap();
    let (status, view) = request(
        &h,
        "GET",
        &format!("/api/films/{}?switch_from={}", target.id, original.id),
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["switch_hint"]["matched"], true);
    assert_eq!(view["switch_hint"]["episode_title"], "第三集");
    assert_eq!(view["switch_hint"]["from_position"], 36.0);
    assert!(view["progress"].is_null(), "换源建议不能写入新来源的进度");
    assert_eq!(catalog::group_key(&original), catalog::group_key(&target));
    let hint = catalog::switch_hint(&h.app, &h.other.user.id, &target.id, &original.id)
        .await
        .unwrap();
    assert_eq!(hint["matched"], false);
    assert!(
        hint["from_position"].is_null(),
        "其他账号不能读取原用户的观看位置"
    );
}

#[tokio::test]
async fn source_switch_requires_selection_for_versions_and_ambiguous_episodes() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let original = catalog::ingest(&h.app, &source, &json!({"vod_id":10,"vod_name":"换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"原线路",
        "vod_play_url":"第03集$https://example.com/3.m3u8"}), true).await.unwrap().unwrap();
    let ep = catalog::episodes(&h.app, &original.id)
        .await
        .unwrap()
        .remove(0);
    sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?)")
        .bind(&h.who.user.id)
        .bind(&original.id)
        .bind(&ep.id)
        .bind(36.0)
        .bind(120.0)
        .bind(1_i64)
        .bind(now())
        .execute(&h.app.db)
        .await
        .unwrap();
    for (id, year, remarks, list) in [
        (20, "2025", "", "第03集$https://example.com/3.m3u8"),
        (
            21,
            "2026",
            "",
            "第01-03集$https://example.com/collection.m3u8",
        ),
        (
            22,
            "2026",
            "",
            "第03集$https://example.com/a.m3u8#第003集$https://example.com/b.m3u8",
        ),
        (
            23,
            "2026",
            "特别篇",
            "第03集$https://example.com/special.m3u8",
        ),
        (24, "", "", "第03集$https://example.com/unknown.m3u8"),
    ] {
        let target = catalog::ingest(&h.app, &source, &json!({"vod_id":id,"vod_name":"换源剧集","vod_year":year,"type_name":"电视剧","vod_remarks":remarks,"vod_play_from":"新线路","vod_play_url":list}), true).await.unwrap().unwrap();
        let hint = catalog::switch_hint(&h.app, &h.who.user.id, &target.id, &original.id)
            .await
            .unwrap();
        assert_eq!(
            hint["matched"], false,
            "不确定的版本或单集必须重新选择：{id}"
        );
        if year != "2026" || !remarks.is_empty() {
            assert_ne!(catalog::group_key(&original), catalog::group_key(&target));
        }
    }
}

#[tokio::test]
async fn ending_with_a_final_snapshot_keeps_it_after_late_reports() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    playback::record(&h.app, &p, 20.0, 120.0, None)
        .await
        .unwrap();
    let (status, _) = request(
        &h,
        "POST",
        &format!("/api/plays/{}/end", p.id),
        json!({"position":64.5,"duration":120.0}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(playback::get(&h.app, &p.id).await.unwrap().ended);
    assert!(
        !playback::record(&h.app, &p, 25.0, 120.0, None)
            .await
            .unwrap()
    );
    let view = catalog::view(&h.app, &h.who.user.id, &h.film, false)
        .await
        .unwrap();
    assert_eq!(view["progress"]["position"], 64.5);
    let resume = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    assert_eq!(resume["position"], 64.5);
}

#[tokio::test]
async fn final_snapshot_and_end_are_atomic_under_concurrent_old_reports() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    let gate = Arc::new(tokio::sync::Barrier::new(33));
    let mut workers = tokio::task::JoinSet::new();
    for n in 0..32 {
        let app = h.app.clone();
        let play = p.clone();
        let gate = gate.clone();
        workers.spawn(async move {
            gate.wait().await;
            playback::record(&app, &play, n as f64, 120.0, None).await
        });
    }
    gate.wait().await;
    playback::end_with_snapshot(&h.app, &p, Some((73.5, 120.0)))
        .await
        .unwrap();
    while let Some(result) = workers.join_next().await {
        result.unwrap().unwrap();
    }
    let view = catalog::view(&h.app, &h.who.user.id, &h.film, false)
        .await
        .unwrap();
    assert_eq!(view["progress"]["position"], 73.5);
    assert!(playback::get(&h.app, &p.id).await.unwrap().ended);
}

#[tokio::test]
async fn force_no_proxy_blocks_new_existing_and_unauthenticated_routes() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    assert_eq!(start["mode"], "direct");
    assert_eq!(h.hits.load(Ordering::Relaxed), 0);
    playback::set_forbidden(&h.app, true).await.unwrap();
    assert_eq!(
        playback::fallback(&h.app, &h.who, &text(&start, "id"), "manual")
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    assert_eq!(p.mode, "direct");
    playback::set_forbidden(&h.app, false).await.unwrap();
    let fallback = playback::fallback(&h.app, &h.who, &p.id, "network")
        .await
        .unwrap();
    playback::set_forbidden(&h.app, true).await.unwrap();
    let parts = text(&fallback, "url");
    let parts = parts.split('/').collect::<Vec<_>>();
    let response = playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new()).await;
    assert_eq!(response.unwrap_err().0, StatusCode::FORBIDDEN);
    assert_eq!(h.hits.load(Ordering::Relaxed), 0);
    let log = playback::logs(&h.app, &h.who, 0).await.unwrap();
    assert!(log.to_string().contains("proxy_blocked"));
    assert!(!log.to_string().contains("access_hash"));
    assert!(!log.to_string().contains("http://"));
    let (status, _) = request(
        &h,
        "PATCH",
        "/api/settings",
        json!({"forbid_proxy":false}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn enabling_no_proxy_interrupts_active_stream() {
    let h = harness().await;
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    let base = url::Url::parse(&e.url).unwrap().join("/slow").unwrap();
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(base.to_string())
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.who, &e.id).await.unwrap();
    let fb = playback::fallback(&h.app, &h.who, &text(&start, "id"), "manual")
        .await
        .unwrap();
    let u = text(&fb, "url");
    let parts = u.split('/').collect::<Vec<_>>();
    let r = playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
        .await
        .unwrap();
    let mut body = r.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(first, bytes::Bytes::from_static(b"first-chunk"));
    playback::set_forbidden(&h.app, true).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(1), body.frame())
        .await
        .expect("禁止转发必须立即中断等待中的资源读取");
    assert!(next.is_none() || next.unwrap().is_err());
}
#[tokio::test]
async fn hls_proxy_rewrites_variants_keys_maps_and_segments() {
    let h = harness().await;
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    let base = url::Url::parse(&e.url).unwrap().join("/master").unwrap();
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(base.to_string())
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.who, &e.id).await.unwrap();
    let fb = playback::fallback(&h.app, &h.who, &text(&start, "id"), "network")
        .await
        .unwrap();
    let u = text(&fb, "url");
    let parts = u.split('/').collect::<Vec<_>>();
    let r = playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
        .await
        .unwrap();
    let body =
        String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let next = body
        .lines()
        .find(|l| l.starts_with("/media/"))
        .unwrap()
        .split('/')
        .collect::<Vec<_>>();
    let mut headers = HeaderMap::new();
    headers.insert("range", "bytes=0-0".parse().unwrap());
    let r = playback::proxy(&h.app, next[2], next[3], Method::GET, headers)
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(!r.headers().contains_key("content-range"));
    let body =
        String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(!body.contains("secret"));
    assert!(!body.contains("key.bin"));
    assert!(body.contains("URI=\"/media/"));
    assert_eq!(body.matches("/media/").count(), 3);
    assert!(body.contains(".key\""));
    assert!(body.contains(".mp4\""));
    assert!(
        body.lines()
            .any(|line| line.starts_with("/media/") && line.ends_with(".ts"))
    );
    let key = body
        .lines()
        .find(|l| l.contains("EXT-X-KEY"))
        .unwrap()
        .split("URI=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let parts = key.split('/').collect::<Vec<_>>();
    assert!(
        playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn large_hls_rewrites_and_repeated_requests_do_not_block_progress() {
    let h = harness().await;
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    let base = url::Url::parse(&e.url)
        .unwrap()
        .join("/large.m3u8")
        .unwrap();
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(base.to_string())
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.who, &e.id).await.unwrap();
    let id = text(&start, "id");
    let fb = playback::fallback(&h.app, &h.who, &id, "manual")
        .await
        .unwrap();
    let p = playback::get(&h.app, &id).await.unwrap();
    let path = text(&fb, "url");
    let mut manifests = Vec::new();
    for _ in 0..3 {
        let app = h.app.clone();
        let path = path.clone();
        manifests.push(tokio::spawn(async move {
            let parts = path.split('/').collect::<Vec<_>>();
            let response = playback::proxy(&app, parts[2], parts[3], Method::GET, HeaderMap::new())
                .await
                .expect("长清单与进度并发时仍能转发");
            assert_eq!(response.status(), StatusCode::OK);
            String::from_utf8(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap()
        }));
    }
    let app = h.app.clone();
    let reports = tokio::spawn(async move {
        for position in 1..=20 {
            assert!(
                playback::record(&app, &p, position as f64, 120.0, None)
                    .await
                    .unwrap()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        for manifest in manifests {
            let body = manifest.await.unwrap();
            assert_eq!(body.matches("#EXTINF:").count(), 2400);
            assert_eq!(body.matches("/media/").count(), 2402);
            assert!(!body.contains("secret"));
        }
        reports.await.unwrap();
    })
    .await
    .expect("长清单交付不应超过客户端重试等待时间");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_assets WHERE play_id=?")
        .bind(&id)
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(count, 2403, "重复请求不能重复登记分片、密钥与初始化段");
    assert_eq!(playback::get(&h.app, &id).await.unwrap().position, 20.0);
}
#[tokio::test]
async fn media_resource_limit_is_atomic_and_existing_resources_remain_accessible() {
    let h = harness().await;
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    let base = url::Url::parse(&e.url)
        .unwrap()
        .join("/large.m3u8")
        .unwrap();
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(base.to_string())
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.who, &e.id).await.unwrap();
    let id = text(&start, "id");
    let fb = playback::fallback(&h.app, &h.who, &id, "manual")
        .await
        .unwrap();
    let p = playback::get(&h.app, &id).await.unwrap();
    sqlx::query("WITH RECURSIVE items(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM items WHERE n<11000) INSERT INTO media_assets SELECT 'bounded-'||n,?,'https://media.example/existing' FROM items")
        .bind(&id).execute(&h.app.db).await.unwrap();
    let path = text(&fb, "url");
    let parts = path.split('/').collect::<Vec<_>>();
    let rejected = playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
        .await
        .unwrap_err();
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_assets WHERE play_id=?")
        .bind(&id)
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(count, 11001, "超额清单不能留下部分分片映射");
    sqlx::query("WITH RECURSIVE items(n) AS (VALUES(11001) UNION ALL SELECT n+1 FROM items WHERE n<11998) INSERT INTO media_assets SELECT 'bounded-'||n,?,'https://media.example/existing' FROM items")
        .bind(&id).execute(&h.app.db).await.unwrap();
    let (first, second) = tokio::join!(
        playback::asset(&h.app, &p, "https://media.example/first"),
        playback::asset(&h.app, &p, "https://media.example/second")
    );
    assert_ne!(first.is_ok(), second.is_ok(), "并发登记必须遵守总量上限");
    assert!(playback::asset(&h.app, &p, base.as_str()).await.is_ok());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_assets WHERE play_id=?")
        .bind(&id)
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(count, 12000);
}
#[tokio::test]
async fn encrypted_hls_metadata_is_probed_from_bounded_local_resources() {
    let h = harness().await;
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    let target = url::Url::parse(&e.url)
        .unwrap()
        .join("/encrypted.m3u8")
        .unwrap();
    sqlx::query("UPDATE episodes SET url=?,probe='{}',probed_at=0 WHERE id=?")
        .bind(target.as_str())
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let e = catalog::episode(&h.app, &e.id).await.unwrap();
    let info = tv_rust::probe::get(&h.app, &e).await.unwrap();
    assert_eq!(info.kind, "hls");
    assert_eq!(info.probe_version, 1);
    assert!(info.duration >= 1.0);
    assert!(
        info.streams
            .iter()
            .any(|v| v["codec_type"] == "video" && v["width"] == 64)
    );
    assert!(
        info.streams
            .iter()
            .any(|v| v["codec_type"] == "audio" && v["codec_name"] == "aac")
    );
    let count = h.hits.load(Ordering::Relaxed);
    let cached = catalog::episode(&h.app, &e.id).await.unwrap();
    assert_eq!(
        tv_rust::probe::get(&h.app, &cached)
            .await
            .unwrap()
            .streams
            .len(),
        info.streams.len()
    );
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        count,
        "缓存命中不重复拉取密钥或分片"
    );
}
#[tokio::test]
async fn range_proxy_preserves_status_and_headers() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let fb = playback::fallback(&h.app, &h.who, &text(&start, "id"), "manual")
        .await
        .unwrap();
    let u = text(&fb, "url");
    let parts = u.split('/').collect::<Vec<_>>();
    let mut headers = HeaderMap::new();
    headers.insert("range", "bytes=2-5".parse().unwrap());
    let r = playback::proxy(&h.app, parts[2], parts[3], Method::GET, headers)
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.headers()["content-range"], "bytes 2-5/8");
    assert_eq!(r.into_body().collect().await.unwrap().to_bytes(), "2345");
}
#[tokio::test]
async fn password_change_revokes_auth_and_media_capabilities() {
    let h = harness().await;
    let start = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let fb = playback::fallback(&h.app, &h.who, &text(&start, "id"), "manual")
        .await
        .unwrap();
    auth::reset_password(&h.app, "admin", "new-password-123")
        .await
        .unwrap();
    let (status, _) = request(&h, "GET", "/api/me", json!({}), Some(&h.raw)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let u = text(&fb, "url");
    let parts = u.split('/').collect::<Vec<_>>();
    assert!(
        playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
            .await
            .is_err()
    );
    assert!(
        auth::login(&h.app, "admin", &hash("new-password-123"), true)
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn fntv_search_and_strm_contract_avoid_redirecting_login_headers() {
    let h = harness().await;
    let (token, _) = auth::login(&h.app, "admin", &hash("password-123"), true)
        .await
        .unwrap();
    let (status, v) = request(
        &h,
        "GET",
        "/v/api/v1/search/list?q=%E5%8E%9F%E7%94%9F",
        json!({}),
        Some(&token),
    )
    .await;
    assert_eq!(status, 200);
    assert!(v["data"].is_array());
    assert_eq!(v["data"][0]["title"], "原生搜索结果 · 测试源");
    let (status, v) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v["data"]["cloud_storage_info"]["cloud_storage_type"], 9001);
    assert_eq!(v["data"]["header"], json!({}));
    assert!(
        v["data"]["direct_link_qualities"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    let r = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(format!("/v/api/v1/media/range/{}", h.episode))
                .header("authorization", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!r.status().is_redirection());
    assert!(!r.headers().contains_key("location"));
}

#[tokio::test]
async fn native_replaying_the_same_url_has_a_new_identity_and_rejects_old_reports() {
    let h = harness().await;
    let episode = catalog::episode(&h.app, &h.episode).await.unwrap();
    let signed = format!("{}?signature={}", episode.url, "a".repeat(700));
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(&signed)
        .bind(&h.episode)
        .execute(&h.app.db)
        .await
        .unwrap();
    let (token, _) = auth::login(&h.app, "admin", &hash("password-123"), true)
        .await
        .unwrap();
    let (_, first) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    let first_link = first["data"]["direct_link_qualities"][0]["url"]
        .as_str()
        .unwrap();
    assert!(first_link.len() > 256, "签名直链可超过传统播放关联标识长度");
    let (status, _) = request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":first_link,"ts":42}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, resume) = request(
        &h,
        "POST",
        "/v/api/v1/play/info",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    assert_eq!(resume["data"]["ts"], 42);
    let (_, second) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    let second_link = second["data"]["direct_link_qualities"][0]["url"]
        .as_str()
        .unwrap();
    assert_ne!(first_link, second_link);
    let mut first_url = url::Url::parse(first_link).unwrap();
    let mut second_url = url::Url::parse(second_link).unwrap();
    first_url.set_fragment(None);
    second_url.set_fragment(None);
    assert_eq!(
        first_url, second_url,
        "重复播放只改变片段，上游 HTTP 地址及签名必须一致"
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":second_link,"ts":12}),
        Some(&token),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":first_link,"ts":90}),
        Some(&token),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &h.film, false)
            .await
            .unwrap()["progress"]["position"],
        12.0
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/quit",
        json!({"play_link":second_link}),
        Some(&token),
    )
    .await;
    let latest: bool = sqlx::query_scalar("SELECT ended FROM plays ORDER BY seq DESC LIMIT 1")
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert!(
        latest,
        "STRM 即使没有调用 play/play，也能按交付标识结束当前会话"
    );
}

#[tokio::test]
async fn native_manual_fallback_survives_late_direct_reports_until_redelivery() {
    let h = harness().await;
    let (token, _) = auth::login(&h.app, "admin", &hash("password-123"), true)
        .await
        .unwrap();
    let (_, direct) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    let direct_link = direct["data"]["direct_link_qualities"][0]["url"]
        .as_str()
        .unwrap();
    let p: playback::Play = sqlx::query_as("SELECT * FROM plays ORDER BY seq DESC LIMIT 1")
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    playback::fallback(&h.app, &h.who, &p.id, "manual")
        .await
        .unwrap();
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":direct_link,"ts":42}),
        Some(&token),
    )
    .await;
    assert_eq!(
        playback::get(&h.app, &p.id).await.unwrap().state,
        "fallback"
    );
    let (status, proxy) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let proxy_link = proxy["data"]["direct_link_qualities"][0]["url"]
        .as_str()
        .unwrap();
    assert!(proxy_link.starts_with("http://127.0.0.1:3000/media/"));
    assert_eq!(playback::get(&h.app, &p.id).await.unwrap().state, "issued");
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":direct_link,"ts":90}),
        Some(&token),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":h.episode,"play_link":proxy_link,"ts":19}),
        Some(&token),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &h.film, false)
            .await
            .unwrap()["progress"]["position"],
        19.0
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/quit",
        json!({"play_link":proxy_link}),
        Some(&token),
    )
    .await;
    let (_, fresh) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&token),
    )
    .await;
    assert!(
        !fresh["data"]["direct_link_qualities"][0]["url"]
            .as_str()
            .unwrap()
            .contains("/media/")
    );
}

#[tokio::test]
async fn default_network_policy_rejects_loopback_before_request() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(dir.path().into(), false, String::new())
        .await
        .unwrap();
    let e = tv_rust::net::request(
        &app,
        Method::GET,
        "http://127.0.0.1:1/private",
        &HeaderMap::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sources")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
