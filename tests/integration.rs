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
use tv_rust::{access, auth, catalog, core::*, playback, preferences, router};

struct Harness {
    _dir: tempfile::TempDir,
    app: App,
    who: Identity,
    other: Identity,
    raw: String,
    member_raw: String,
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
            "/image-prefixed.m3u8"=>([("content-type","application/vnd.apple.mpegurl")],"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nimage-prefixed.ts\n#EXT-X-ENDLIST\n").into_response(),
            "/image-only.m3u8"=>([("content-type","application/vnd.apple.mpegurl")],"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\npicture.png\n#EXT-X-ENDLIST\n").into_response(),
            "/image-prefixed.ts"=>([("content-type","image/png")],include_bytes!("fixtures/hls-probe/image-prefixed.ts").as_slice()).into_response(),
            "/picture.png"=>([("content-type","image/png")],&include_bytes!("fixtures/hls-probe/image-prefixed.ts")[..112]).into_response(),
            "/gone.m3u8"=>StatusCode::GONE.into_response(),
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
            "/switch-api"=>axum::Json(json!({"list":[
                {"vod_id":401,"vod_name":"测试电影","vod_year":"2026","vod_play_from":"备用线路","vod_play_url":"正片$https://example.com/movie.mp4"},
                {"vod_id":402,"vod_name":"测试电影","vod_year":"2025"},
                {"vod_id":403,"vod_name":"测试电影","vod_year":"2026","vod_remarks":"导演剪辑"},
                {"vod_id":404,"vod_name":"测试电影外传","vod_year":"2026"}
            ],"pagecount":1})).into_response(),
            "/quality-api"=>{
                let host=req.headers().get("host").unwrap().to_str().unwrap();
                axum::Json(json!({"list":[{"vod_id":1,"vod_name":"测试电影","vod_year":"2026","vod_play_from":"line","vod_play_url":format!("正片$http://{host}/encrypted.m3u8")}]})).into_response()
            },
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
        auth_hash: hash(&other_raw),
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
        member_raw: other_raw,
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

async fn extra_member(h: &Harness) -> User {
    auth::create_user(
        &h.app,
        &h.who,
        &json!({"username":"extra","password":"extra-password-123"}),
    )
    .await
    .unwrap();
    sqlx::query_as("SELECT id,username,admin,disabled FROM users WHERE username='extra'")
        .fetch_one(&h.app.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn image_preferences_are_saved_per_account_and_survive_restart() {
    let h = harness().await;
    let (_, initial) = request(&h, "GET", "/api/me", json!({}), Some(&h.member_raw)).await;
    assert_eq!(
        initial["preferences"]["douban_image_source"],
        "cmliussss-cdn-tencent"
    );
    let value = json!({"douban_image_source":"custom","douban_image_custom_url":"https://images.example/fetch?url="});
    assert_eq!(
        request(
            &h,
            "PATCH",
            "/api/me/preferences",
            value.clone(),
            Some(&h.member_raw)
        )
        .await
        .0,
        200
    );
    let (_, saved) = request(&h, "GET", "/api/me", json!({}), Some(&h.member_raw)).await;
    assert_eq!(saved["preferences"], value);
    let (_, admin) = request(&h, "GET", "/api/me", json!({}), Some(&h.raw)).await;
    assert_eq!(
        admin["preferences"]["douban_image_source"],
        "cmliussss-cdn-tencent"
    );
    for invalid in [
        json!({"douban_image_source":"server"}),
        json!({"douban_image_source":"custom","douban_image_custom_url":""}),
        json!({"douban_image_source":"custom","douban_image_custom_url":"javascript:alert(1)"}),
        json!({"douban_image_source":"custom","douban_image_custom_url":"https://user:pass@images.example/fetch?url="}),
        json!({"douban_image_source":"cmliussss-cdn-ali","user_id":h.who.user.id}),
    ] {
        assert_eq!(
            request(
                &h,
                "PATCH",
                "/api/me/preferences",
                invalid,
                Some(&h.member_raw)
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        request(&h, "PATCH", "/api/me/preferences", value.clone(), None)
            .await
            .0,
        401
    );
    let restart = App::new(h._dir.path().into(), true, String::new())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(preferences::get(&restart, &h.other.user.id).await.unwrap()).unwrap(),
        value
    );
}

#[tokio::test]
async fn subscription_usage_is_atomic_admin_only_and_preserved_by_updates() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let path = format!("/api/subscriptions/{}", source.subscription_id);
    let usage = json!({"mode":"selected","user_ids":[h.other.user.id]});
    assert_eq!(
        request(
            &h,
            "PATCH",
            &path,
            json!({"usage":usage}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    for invalid in [
        json!({"mode":"selected","user_ids":[]}),
        json!({"mode":"selected","user_ids":["missing"]}),
        json!({"mode":"all","user_ids":[h.other.user.id]}),
        json!({"mode":"admins","user_ids":[h.other.user.id]}),
        json!({"mode":"selected","user_ids":[h.who.user.id]}),
    ] {
        assert_eq!(
            request(&h, "PATCH", &path, json!({"usage":invalid}), Some(&h.raw))
                .await
                .0,
            400
        );
    }
    assert_eq!(
        catalog::subscription_list(&h.app).await.unwrap()["subscriptions"][0]["usage"]["mode"],
        "all"
    );
    let hits = h.hits.load(Ordering::Relaxed);
    assert_eq!(
        request(&h, "PATCH", &path, json!({"usage":usage}), Some(&h.raw))
            .await
            .0,
        200
    );
    assert_eq!(h.hits.load(Ordering::Relaxed), hits, "修改用户不能下载配置");
    assert!(
        access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    let extra = extra_member(&h).await;
    assert!(
        !access::source_allowed(&h.app, &extra.id, &source.id)
            .await
            .unwrap()
    );
    assert!(
        access::source_allowed(&h.app, &h.who.user.id, &source.id)
            .await
            .unwrap()
    );
    let config =
        json!({"api_site":{"same.key":{"name":"保留授权源","api":source.api}}}).to_string();
    catalog::import(
        &h.app,
        &json!({"id":source.subscription_id,"name":"更新订阅","content":config}),
    )
    .await
    .unwrap();
    let listing = catalog::subscription_list(&h.app).await.unwrap();
    assert_eq!(listing["subscriptions"][0]["usage"], usage);
    sqlx::query("UPDATE subscriptions SET url=? WHERE id=?")
        .bind(
            url::Url::parse(&source.api)
                .unwrap()
                .join("/config")
                .unwrap()
                .to_string(),
        )
        .bind(&source.subscription_id)
        .execute(&h.app.db)
        .await
        .unwrap();
    catalog::refresh(&h.app, &source.subscription_id)
        .await
        .unwrap();
    assert_eq!(
        catalog::subscription_list(&h.app).await.unwrap()["subscriptions"][0]["usage"],
        usage
    );
    sqlx::query("UPDATE users SET disabled=1 WHERE id=?")
        .bind(&h.other.user.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    assert!(
        !access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE users SET disabled=0 WHERE id=?")
        .bind(&h.other.user.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    assert!(
        access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    assert_eq!(
        request(
            &h,
            "PATCH",
            &path,
            json!({"usage":{"mode":"all","user_ids":[]}}),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    assert!(
        access::source_allowed(&h.app, &extra.id, &source.id)
            .await
            .unwrap(),
        "所有用户包含后来创建的账号"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions")
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert!(catalog::import(&h.app,&json!({"name":"无效分配","content":config,"usage":{"mode":"selected","user_ids":["missing"]}})).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscriptions")
            .fetch_one(&h.app.db)
            .await
            .unwrap(),
        count
    );
}

#[tokio::test]
async fn admins_only_subscriptions_support_import_edit_refresh_and_restart() {
    let h = harness().await;
    let original = catalog::sources(&h.app).await.unwrap().remove(0);
    let config =
        json!({"api_site":{"same.key":{"name":"管理员专用源","api":original.api}}}).to_string();
    let admins = json!({"mode":"admins","user_ids":[]});
    let (status, imported) = request(
        &h,
        "POST",
        "/api/subscriptions",
        json!({"name":"管理员订阅","content":config,"usage":admins}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    let id = text(&imported, "id");
    let source = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .find(|source| source.subscription_id == id)
        .unwrap();
    let listing = catalog::subscription_list(&h.app).await.unwrap();
    assert_eq!(
        listing["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sub| sub["id"] == id)
            .unwrap()["usage"],
        admins
    );
    assert!(
        access::source_allowed(&h.app, &h.who.user.id, &source.id)
            .await
            .unwrap()
    );
    assert!(
        !access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    assert!(
        !access::sources(&h.app, &h.other.user.id)
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == source.id)
    );
    auth::create_user(
        &h.app,
        &h.who,
        &json!({"username":"admin-two","password":"second-admin-password","admin":true}),
    )
    .await
    .unwrap();
    let second_admin: String =
        sqlx::query_scalar("SELECT id FROM users WHERE username='admin-two'")
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    assert!(
        access::source_allowed(&h.app, &second_admin, &source.id)
            .await
            .unwrap(),
        "仅管理员允许其他管理员账号使用"
    );
    let later = extra_member(&h).await;
    assert!(
        !access::source_allowed(&h.app, &later.id, &source.id)
            .await
            .unwrap(),
        "新建普通成员不获得仅管理员订阅"
    );
    let path = format!("/api/subscriptions/{id}");
    assert_eq!(
        request(
            &h,
            "PATCH",
            &path,
            json!({"usage":admins}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    access::save_usage(
        &h.app,
        &id,
        &json!({"mode":"selected","user_ids":[h.other.user.id]}),
    )
    .await
    .unwrap();
    assert!(
        access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    let hits = h.hits.load(Ordering::Relaxed);
    assert_eq!(
        request(&h, "PATCH", &path, json!({"usage":admins}), Some(&h.raw))
            .await
            .0,
        200
    );
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "改为仅管理员无需下载订阅"
    );
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscription_users WHERE subscription_id=?")
            .bind(&id)
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    assert_eq!(grants, 0, "切换到仅管理员清除旧成员授权");
    assert!(
        !access::source_allowed(&h.app, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    catalog::import(
        &h.app,
        &json!({"id":id,"name":"管理员订阅","content":config}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE subscriptions SET url=? WHERE id=?")
        .bind(
            url::Url::parse(&original.api)
                .unwrap()
                .join("/config")
                .unwrap()
                .to_string(),
        )
        .bind(&id)
        .execute(&h.app.db)
        .await
        .unwrap();
    catalog::refresh(&h.app, &id).await.unwrap();
    let restarted = App::new(h._dir.path().into(), true, String::new())
        .await
        .unwrap();
    let listing = catalog::subscription_list(&restarted).await.unwrap();
    assert_eq!(
        listing["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sub| sub["id"] == id)
            .unwrap()["usage"],
        admins
    );
    assert!(
        !access::source_allowed(&restarted, &h.other.user.id, &source.id)
            .await
            .unwrap()
    );
    assert!(
        access::source_allowed(&restarted, &second_admin, &source.id)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&restarted.db)
            .await
            .unwrap(),
        6
    );
}

#[tokio::test]
async fn revoked_subscription_blocks_web_native_caches_posters_and_media() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    sqlx::query("UPDATE films SET poster=? WHERE id=?")
        .bind("https://images.example/private-cover.jpg")
        .bind(&h.film)
        .execute(&h.app.db)
        .await
        .unwrap();
    catalog::favorite(&h.app, &h.other.user.id, &h.film, true)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.other, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&start, "id")).await.unwrap();
    playback::record(&h.app, &p, 42.0, 120.0, None)
        .await
        .unwrap();
    let fallback = playback::fallback(&h.app, &h.other, &p.id, "manual")
        .await
        .unwrap();
    let warmed = catalog::search(&h.app, &h.other.user.id, "原生搜索结果", 1, None)
        .await
        .unwrap();
    assert_eq!(warmed["items"].as_array().unwrap().len(), 1);
    // 即便旧响应再次进入相同权限集合的缓存，命中时也要重新过滤源。
    let prior_cache: (String, String) =
        sqlx::query_as("SELECT key,value FROM cache WHERE key LIKE 'search:%'")
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    app_seed_recommendation(&h.app, "123", "测试电影").await;
    h.app
        .cache_put(
            "recommendation-meta:v1:123",
            &json!({"year":"2026","kind":"Movie"}),
            3600,
        )
        .await
        .unwrap();
    assert!(
        !catalog::recommendation_posters(&h.app, &h.other.user.id, "123", "movie")
            .await
            .unwrap()["urls"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    let hits = h.hits.load(Ordering::Relaxed);
    let (status, sources) =
        request(&h, "GET", "/api/sources", json!({}), Some(&h.member_raw)).await;
    assert_eq!(status, 200);
    assert!(sources["sources"].as_array().unwrap().is_empty());
    let (_, search) = request(
        &h,
        "GET",
        "/api/search?q=原生搜索结果",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert!(search["items"].as_array().unwrap().is_empty());
    assert!(search["sources"].as_array().unwrap().is_empty());
    assert_eq!(
        request(
            &h,
            "GET",
            &format!("/api/films/{}?refresh=1", h.film),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    for path in [
        format!("/api/films/{}/posters", h.film),
        format!("/api/posters/{}", h.film),
        format!("/tv-poster/{}", h.film),
        format!("/v/api/v1/sys/img/tv-poster/{}", h.film),
        format!("/v/api/v1/item/{}", h.film),
        format!("/v/api/v1/season/list/{}", h.film),
        format!("/v/api/v1/episode/list/{}", tv_rust::fntv::season(&h.film)),
        format!("/v/api/v1/stream/list/{}", h.episode),
        format!("/v/api/v1/media/range/{}", h.episode),
    ] {
        assert_eq!(
            request(&h, "GET", &path, json!({}), Some(&h.member_raw))
                .await
                .0,
            403,
            "{path}"
        );
    }
    assert_eq!(
        request(
            &h,
            "POST",
            "/api/plays",
            json!({"episode_id":h.episode}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            &h,
            "PUT",
            &format!("/api/films/{}/favorite", h.film),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    for path in [
        "/v/api/v1/play/info",
        "/v/api/v1/stream",
        "/v/api/v1/stream/directLink",
        "/v/api/v1/play/play",
        "/v/api/v1/play/record",
    ] {
        assert_eq!(
            request(
                &h,
                "POST",
                path,
                json!({"media_guid":h.episode,"item_guid":h.film,"ts":55}),
                Some(&h.member_raw)
            )
            .await
            .0,
            403,
            "{path}"
        );
    }
    let (_, native) = request(
        &h,
        "GET",
        "/v/api/v1/search/list?q=原生搜索结果",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert!(native["data"].as_array().unwrap().is_empty());
    assert_eq!(
        catalog::recommendation_posters(&h.app, &h.other.user.id, "123", "movie")
            .await
            .unwrap(),
        json!({"urls":[]})
    );
    assert_eq!(
        request(&h, "GET", &text(&fallback, "url"), json!({}), None)
            .await
            .0,
        403
    );
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "未授权请求不能访问上游"
    );
    let stale = playback::get(&h.app, &p.id).await.unwrap();
    assert!(stale.ended);
    assert_eq!(stale.state, "access_revoked");
    assert!(
        playback::record(&h.app, &p, 99.0, 120.0, None)
            .await
            .is_err()
    );
    playback::end_with_snapshot(&h.app, &p, Some((99.0, 120.0)))
        .await
        .unwrap();
    for kind in ["favorites", "history"] {
        let personal = catalog::personal(&h.app, &h.other.user.id, kind)
            .await
            .unwrap();
        assert_eq!(personal[0]["available"], false);
        assert_eq!(personal[0]["authorized"], false);
        assert_eq!(
            personal[0]["unavailable_reason"],
            access::UNAUTHORIZED_SOURCE
        );
        assert_eq!(personal[0]["poster"], "");
        assert_eq!(personal[0]["progress"]["position"], 42.0);
    }
    let (_, native_record) = request(
        &h,
        "POST",
        "/v/api/v1/favorite/list",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(native_record["data"]["list"][0]["can_play"], 0);
    assert_eq!(native_record["data"]["list"][0]["poster"], "");
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"all","user_ids":[]}),
    )
    .await
    .unwrap();
    h.app
        .cache_put(
            &prior_cache.0,
            &serde_json::from_str(&prior_cache.1).unwrap(),
            180,
        )
        .await
        .unwrap();
    assert_eq!(
        catalog::search(&h.app, &h.other.user.id, "原生搜索结果", 1, None)
            .await
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()[0]["available"],
        true
    );
    assert!(
        playback::get(&h.app, &p.id).await.unwrap().ended,
        "重新授权不恢复旧播放令牌"
    );
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &h,
            "DELETE",
            &format!("/api/films/{}/favorite", h.film),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &h,
            "DELETE",
            &format!("/api/films/{}/history", h.film),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        200
    );
    assert!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revoking_access_interrupts_a_waiting_stream_without_affecting_admin() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let e = catalog::episode(&h.app, &h.episode).await.unwrap();
    sqlx::query("UPDATE episodes SET url=? WHERE id=?")
        .bind(
            url::Url::parse(&e.url)
                .unwrap()
                .join("/slow")
                .unwrap()
                .to_string(),
        )
        .bind(&e.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let start = playback::start(&h.app, &h.other, &e.id).await.unwrap();
    let fb = playback::fallback(&h.app, &h.other, &text(&start, "id"), "manual")
        .await
        .unwrap();
    let url = text(&fb, "url");
    let parts: Vec<_> = url.split('/').collect();
    let response = playback::proxy(&h.app, parts[2], parts[3], Method::GET, HeaderMap::new())
        .await
        .unwrap();
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        bytes::Bytes::from_static(b"first-chunk")
    );
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(1), body.frame())
        .await
        .expect("撤权必须立即中断等待中的媒体读取");
    assert!(next.is_none() || next.unwrap().is_err());
    assert!(
        playback::start(&h.app, &h.who, &e.id).await.is_ok(),
        "管理员始终可以使用订阅"
    );
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
        6
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
    let result = catalog::search(&h.app, &h.who.user.id, "你好", 1, None)
        .await
        .unwrap();
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
        catalog::search(&h.app, &h.who.user.id, "你好", 1, None)
            .await
            .unwrap(),
        result
    );
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
    let nba = catalog::search(&h.app, &h.who.user.id, "ＮＢＡ", 1, None)
        .await
        .unwrap();
    assert_eq!(nba["items"].as_array().unwrap().len(), 1);
    assert!(text(&nba["items"][0], "title").contains("NBA"));
    let words = catalog::search(&h.app, &h.who.user.id, "你好 乔安", 1, None)
        .await
        .unwrap();
    assert_eq!(words["items"].as_array().unwrap().len(), 1);
    assert_eq!(words["items"][0]["search_rank"], 2);
    let empty = catalog::search(&h.app, &h.who.user.id, "完全不相关", 1, None)
        .await
        .unwrap();
    assert!(empty["items"].as_array().unwrap().is_empty());
    assert_eq!(empty["has_more"], true);
    assert_eq!(
        catalog::search(&h.app, &h.who.user.id, "完全不相关", 500, None)
            .await
            .unwrap()["has_more"],
        false
    );
    assert!(
        catalog::search(&h.app, &h.who.user.id, "!!!", 1, None)
            .await
            .is_err()
    );
    let (status, native) = request(
        &h,
        "POST",
        "/v/api/v1/item/list",
        json!({"q":"你好","page":1,"page_size":50}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(native["data"]["list"][0]["title"], "你好");
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
    let candidates = catalog::film_posters(&h.app, &h.who.user.id, &primary.id)
        .await
        .unwrap();
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
        catalog::film_posters(&h.app, &h.who.user.id, &primary.id)
            .await
            .unwrap(),
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

async fn watched_film(
    h: &Harness,
    source: &catalog::Source,
    row: Value,
    progress: (&str, f64, i64, i64),
) -> catalog::Film {
    let film = catalog::ingest(&h.app, source, &row, true)
        .await
        .unwrap()
        .unwrap();
    let episode = catalog::episodes(&h.app, &film.id).await.unwrap().remove(0);
    sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?)")
        .bind(progress.0)
        .bind(&film.id)
        .bind(&episode.id)
        .bind(progress.1)
        .bind(120.0)
        .bind(progress.2)
        .bind(progress.3)
        .execute(&h.app.db)
        .await
        .unwrap();
    film
}

#[tokio::test]
async fn continue_watching_groups_sources_without_merging_progress_or_user_records() {
    let h = harness().await;
    let primary = catalog::sources(&h.app).await.unwrap().remove(0);
    let subscription = catalog::import(
        &h.app,
        &json!({"name":"继续观看订阅","content":json!({"api_site":{
        "a":{"name":"观看源A","api":primary.api},
        "b":{"name":"观看源B","api":primary.api},
        "c":{"name":"观看源C","api":primary.api}
    }}).to_string()}),
    )
    .await
    .unwrap();
    let sources: Vec<_> = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .filter(|source| source.subscription_id == subscription)
        .collect();
    let mut films = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let film = watched_film(&h, source, json!({"vod_id":901,"vod_name":"午夜规则","vod_year":"2026","type_name":"电视剧","vod_pic":"https://example.com/poster.jpg","vod_play_from":"line","vod_play_url":"第03集$https://example.com/3.mp4"}),
            (&h.other.user.id, [2.0,53.0,12.0][index], index as i64+1, now()-30+index as i64)).await;
        catalog::favorite(&h.app, &h.other.user.id, &film.id, true)
            .await
            .unwrap();
        films.push(film);
    }
    let (_, before) = request(&h, "GET", "/api/history", json!({}), Some(&h.member_raw)).await;
    let (status, grouped) = request(
        &h,
        "GET",
        "/api/continue-watching",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(grouped["items"].as_array().unwrap().len(), 1);
    assert_eq!(grouped["items"][0]["id"], films[2].id);
    assert_eq!(
        grouped["items"][0]["progress"]["position"], 12.0,
        "选择最近观看记录，不能选进度最大的53秒来源"
    );
    assert_eq!(before["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .len(),
        3
    );
    let (_, after) = request(&h, "GET", "/api/history", json!({}), Some(&h.member_raw)).await;
    assert_eq!(before, after, "两个观看入口采用相同的作品合并规则");
    let (_, favorites) = request(&h, "GET", "/api/favorites", json!({}), Some(&h.member_raw)).await;
    assert_eq!(favorites["items"].as_array().unwrap().len(), 3);
    let (_, admin) = request(&h, "GET", "/api/continue-watching", json!({}), Some(&h.raw)).await;
    assert!(
        admin["items"].as_array().unwrap().is_empty(),
        "只读取当前账号的记录"
    );
    assert_eq!(
        request(&h, "GET", "/api/continue-watching", json!({}), None)
            .await
            .0,
        401
    );
    access::save_usage(
        &h.app,
        &subscription,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    let (_, revoked) = request(
        &h,
        "GET",
        "/api/continue-watching",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(revoked["items"].as_array().unwrap().len(), 1);
    assert_eq!(revoked["items"][0]["id"], films[2].id);
    assert_eq!(revoked["items"][0]["authorized"], false);
    assert_eq!(revoked["items"][0]["poster"], "");
    assert_eq!(revoked["items"][0]["progress"]["position"], 12.0);
}

#[tokio::test]
async fn continue_watching_distinguishes_versions_and_uses_latest_session_within_one_second() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let descriptions = [
        ("午夜规则", "2026", "电视剧", "", 53.0, 1),
        ("午夜规则", "2026", "电视剧", "", 2.0, 2),
        ("午夜规则", "2025", "电视剧", "", 12.0, 3),
        ("午夜规则", "2026", "电影", "", 12.0, 4),
        ("午夜规则", "2026", "电视剧", "导演剪辑", 12.0, 5),
        ("年份未知作品", "", "电视剧", "", 12.0, 6),
        ("年份未知作品", "", "电视剧", "", 12.0, 7),
    ];
    let mut films = Vec::new();
    let at = now();
    for (index, (title, year, kind, remarks, position, authority)) in
        descriptions.into_iter().enumerate()
    {
        films.push(watched_film(&h,&source,json!({"vod_id":910+index,"vod_name":title,"vod_year":year,"type_name":kind,"vod_remarks":remarks,"vod_play_from":"line","vod_play_url":"第03集$https://example.com/3.mp4"}),
            (&h.who.user.id,position,authority,at)).await);
    }
    let grouped = catalog::continue_watching(&h.app, &h.who.user.id)
        .await
        .unwrap();
    assert_eq!(grouped.len(), 6);
    assert!(!grouped.iter().any(|item| item["id"] == films[0].id));
    for film in &films[1..] {
        assert!(
            grouped.iter().any(|item| item["id"] == film.id),
            "不同年份、类别、版本或无法确认年份的条目独立显示"
        );
    }
    assert_eq!(
        grouped
            .iter()
            .find(|item| item["id"] == films[1].id)
            .unwrap()["progress"]["position"],
        2.0
    );
    assert_eq!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .len(),
        7
    );
}

#[tokio::test]
async fn continue_watching_deduplicates_before_applying_the_homepage_limit() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let at = now();
    let mut most_recent = String::new();
    for index in 0..10 {
        most_recent = watched_film(&h,&source,json!({"vod_id":930+index,"vod_name":"午夜规则","vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":"第03集$https://example.com/3.mp4"}),
            (&h.who.user.id,index as f64,index,at)).await.id;
    }
    for index in 0..6 {
        watched_film(&h,&source,json!({"vod_id":950+index,"vod_name":format!("其他作品{index}"),"vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":"第01集$https://example.com/1.mp4"}),
            (&h.who.user.id,12.0,10+index,at-1-index)).await;
    }
    let grouped = catalog::continue_watching(&h.app, &h.who.user.id)
        .await
        .unwrap();
    assert_eq!(grouped.len(), 6, "重复来源不能占满首页的六张卡片");
    assert_eq!(grouped[0]["id"], most_recent);
    assert_eq!(
        grouped
            .iter()
            .filter(|item| item["title"] == "午夜规则")
            .count(),
        1
    );
    assert_eq!(grouped[5]["title"], "其他作品4");
    assert!(!grouped.iter().any(|item| item["title"] == "其他作品5"));
    assert_eq!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .len(),
        16
    );
    let concurrent = futures_util::future::join_all(
        (0..8).map(|_| catalog::continue_watching(&h.app, &h.who.user.id)),
    );
    for result in tokio::time::timeout(Duration::from_secs(3), concurrent)
        .await
        .expect("并发打开首页不得占满连接池并阻塞详情读取")
    {
        assert_eq!(result.unwrap(), grouped);
    }
}

#[tokio::test]
async fn playback_sources_discover_only_the_same_work_and_obey_access() {
    let h = harness().await;
    let primary = catalog::sources(&h.app).await.unwrap().remove(0);
    let api = url::Url::parse(&primary.api)
        .unwrap()
        .join("/switch-api")
        .unwrap()
        .to_string();
    let subscription = catalog::import(&h.app, &json!({"name":"换源订阅","content":json!({"api_site":{"backup":{"name":"备用源","api":api}}}).to_string()})).await.unwrap();
    let path = format!("/api/films/{}/sources", h.film);
    let hits = h.hits.load(Ordering::Relaxed);
    let (status, cached) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(status, 200);
    assert_eq!(cached["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "已保存来源列表无需上游请求"
    );
    let (_, discovered) = request(
        &h,
        "GET",
        &format!("{path}?refresh=1"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(
        discovered["items"].as_array().unwrap().len(),
        2,
        "排除年份不同、剪辑不同和片名包含匹配"
    );
    let discovered_hits = h.hits.load(Ordering::Relaxed);
    request(
        &h,
        "GET",
        &format!("{path}?discover=1"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        discovered_hits,
        "首次展开可复用搜索缓存"
    );
    request(
        &h,
        "GET",
        &format!("{path}?refresh=1"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert!(
        h.hits.load(Ordering::Relaxed) > discovered_hits,
        "手动刷新必须重新查询上游"
    );
    let backup = discovered["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["source_name"] == "备用源")
        .unwrap();
    assert_eq!(backup["title"], "测试电影");
    assert!(backup["episodes"].is_null(), "来源列表不交付媒体 URL");
    let episode = catalog::episodes(&h.app, &h.film).await.unwrap().remove(0);
    let (status, view) = request(
        &h,
        "GET",
        &format!(
            "/api/films/{}?switch_from={}&switch_episode={}",
            text(backup, "id"),
            h.film,
            episode.id
        ),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(view["switch_hint"]["matched"], true);
    assert_eq!(view["switch_hint"]["episode_title"], "正片");
    access::save_usage(
        &h.app,
        &subscription,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    let (_, revoked) = request(
        &h,
        "GET",
        &format!("{path}?refresh=1"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(revoked["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        request(
            &h,
            "GET",
            &format!(
                "/api/films/{}?switch_from={}&switch_episode={}",
                text(backup, "id"),
                h.film,
                episode.id
            ),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    assert_eq!(request(&h, "GET", &path, json!({}), None).await.0, 401);
    access::save_usage(
        &h.app,
        &primary.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    assert_eq!(
        request(&h, "GET", &path, json!({}), Some(&h.member_raw))
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn quick_switch_matches_the_current_episode_without_saved_progress() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let original = catalog::ingest(&h.app,&source,&json!({"vod_id":701,"vod_name":"快速换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"原线路","vod_play_url":"第01集$https://example.com/1.m3u8#第03集$https://example.com/3.m3u8"}),true).await.unwrap().unwrap();
    let target = catalog::ingest(&h.app,&source,&json!({"vod_id":702,"vod_name":"快速换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"普通$$$重复线路","vod_play_url":"第一集$https://example.com/one.m3u8#第三集$https://example.com/three.m3u8$$$第03集$https://example.com/a.m3u8#第003集$https://example.com/b.m3u8"}),true).await.unwrap().unwrap();
    let episode = catalog::episodes(&h.app, &original.id)
        .await
        .unwrap()
        .remove(1);
    let path = format!(
        "/api/films/{}?switch_from={}&switch_episode={}",
        target.id, original.id, episode.id
    );
    let (status, view) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(status, 200);
    assert_eq!(view["switch_hint"]["matched"], true);
    assert_eq!(
        view["switch_hint"]["episode_title"], "第三集",
        "按实际标签匹配而非列表顺序"
    );
    assert!(view["progress"].is_null(), "快速换源不复制个人进度");
    let (_, ambiguous) = request(
        &h,
        "GET",
        &format!(
            "{path}&switch_line={}",
            url::form_urlencoded::byte_serialize("重复线路".as_bytes()).collect::<String>()
        ),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(ambiguous["switch_hint"]["matched"], false);
    let invalid = format!(
        "/api/films/{}?switch_from={}&switch_episode={}",
        target.id, original.id, h.episode
    );
    assert_eq!(
        request(&h, "GET", &invalid, json!({}), Some(&h.member_raw))
            .await
            .0,
        400
    );
}

#[tokio::test]
async fn playback_reports_measured_quality_without_fabricating_unknown_streams() {
    let h = harness().await;
    let unknown = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    assert_eq!(unknown["quality"]["codec"], "");
    assert_eq!(unknown["quality"]["height"].as_f64(), Some(0.0));
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(json!({"kind":"mp4","duration":120,"size":8,"warning":"","streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001","bit_rate":"5000000"}]}).to_string())
        .bind(now()).bind(&h.episode).execute(&h.app.db).await.unwrap();
    let (status, measured) = request(
        &h,
        "POST",
        "/api/plays",
        json!({"episode_id":h.episode}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(measured["quality"]["width"].as_f64(), Some(1920.0));
    assert_eq!(measured["quality"]["height"].as_f64(), Some(1080.0));
    assert_eq!(measured["quality"]["codec"], "h264");
    assert!((measured["quality"]["frame_rate"].as_f64().unwrap() - 29.97).abs() < 0.01);
    assert_eq!(measured["source_name"], "测试源");
    assert_eq!(measured["line_name"], "line");
    assert_eq!(measured["episode_title"], "正片");
}

#[tokio::test]
async fn image_prefixed_hls_replaces_cached_picture_metadata_with_real_video() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let url = url::Url::parse(&catalog::episode(&h.app, &h.episode).await.unwrap().url)
        .unwrap()
        .join("/image-prefixed.m3u8")
        .unwrap();
    let film = catalog::ingest(&h.app, &source, &json!({"vod_id":7200,"vod_name":"图片头媒体识别验收","vod_year":"2026","vod_play_from":"lzm3u8","vod_play_url":format!("正片${url}")}), true).await.unwrap().unwrap();
    let e = catalog::episodes(&h.app, &film.id).await.unwrap().remove(0);
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(json!({"probe_version":1,"kind":"hls","duration":4.0,"size":0,"warning":"","streams":[{"index":0,"codec_type":"video","codec_name":"png","width":28,"height":28,"avg_frame_rate":"25/1"}]}).to_string())
        .bind(now()).bind(&e.id).execute(&h.app.db).await.unwrap();
    let e = catalog::episode(&h.app, &e.id).await.unwrap();
    let info = tv_rust::probe::get(&h.app, &e).await.unwrap();
    let quality = tv_rust::probe::quality(&info);
    assert_eq!(info.probe_version, 2);
    assert_eq!(quality["codec"], "h264");
    assert_eq!(quality["width"].as_f64(), Some(64.0));
    assert_eq!(quality["height"].as_f64(), Some(64.0));
    playback::set_forbidden(&h.app, true).await.unwrap();
    let (status, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":e.id}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    let file_name = text(&stream["data"]["file_stream"], "file_name");
    assert!(file_name.contains("64p · 64 × 64 · H.264"));
    assert!(!file_name.contains("28p"));
    assert!(!file_name.contains("PNG"));
}

#[tokio::test]
async fn image_only_addresses_and_hls_samples_fail_before_native_playback() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let base = url::Url::parse(&catalog::episode(&h.app, &h.episode).await.unwrap().url).unwrap();
    for (id, path) in [(7201, "/image-only.m3u8"), (7202, "/picture.png")] {
        let url = base.join(path).unwrap();
        let film = catalog::ingest(&h.app, &source, &json!({"vod_id":id,"vod_name":format!("图片误用验收{id}"),"vod_year":"2026","vod_play_from":"line","vod_play_url":format!("正片${url}")}), true).await.unwrap().unwrap();
        let e = catalog::episodes(&h.app, &film.id).await.unwrap().remove(0);
        let error = playback::start(&h.app, &h.who, &e.id).await.unwrap_err();
        assert!(error.1.contains("返回图片"));
        let (status, response) = request(
            &h,
            "POST",
            "/v/api/v1/stream",
            json!({"media_guid":e.id}),
            Some(&h.raw),
        )
        .await;
        assert_eq!(status, 400);
        assert!(text(&response, "msg").contains("返回图片"));
    }
}

#[tokio::test]
async fn attached_cover_images_are_not_selected_as_video_quality_or_native_tracks() {
    let h = harness().await;
    let info = json!({"probe_version":2,"kind":"mp4","duration":120.0,"size":8,"warning":"","streams":[
        {"index":0,"codec_type":"video","codec_name":"png","width":28,"height":28,"disposition":{"attached_pic":1}},
        {"index":1,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"24/1"}
    ]});
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(info.to_string())
        .bind(now())
        .bind(&h.episode)
        .execute(&h.app.db)
        .await
        .unwrap();
    let started = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    assert_eq!(started["quality"]["codec"], "h264");
    assert_eq!(started["quality"]["height"].as_f64(), Some(1080.0));
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(stream["data"]["video_stream"]["index"], 1);
    assert_eq!(stream["data"]["video_stream"]["codec_name"], "h264");
    let (_, files) = request(
        &h,
        "GET",
        &format!("/v/api/v1/stream/list/{}", h.episode),
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(files["data"]["video_streams"].as_array().unwrap().len(), 1);
    assert_eq!(files["data"]["video_streams"][0]["codec_name"], "h264");
}

#[tokio::test]
async fn source_quality_uses_the_matching_episode_and_selects_its_measured_best_line() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let original = catalog::ingest(&h.app, &source, &json!({"vod_id":801,"vod_name":"画质对照剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"原线路","vod_play_url":"第01集$https://example.com/1.mp4#第03集$https://example.com/3.mp4"}), true).await.unwrap().unwrap();
    let target = catalog::ingest(&h.app, &source, &json!({"vod_id":802,"vod_name":"画质对照剧集","vod_year":"2026","type_name":"电视剧","vod_remarks":"4K蓝光","vod_play_from":"liangzi$$$lzm3u8","vod_play_url":"第一集$https://example.com/first.mp4#第三集$https://example.com/low.mp4$$$第一集$https://example.com/first-high.mp4#第三集$https://example.com/high.mp4"}), true).await.unwrap().unwrap();
    let current = catalog::episodes(&h.app, &original.id)
        .await
        .unwrap()
        .remove(1);
    let episodes = catalog::episodes(&h.app, &target.id).await.unwrap();
    let cache = |width, height| {
        json!({"kind":"mp4","duration":120,"size":8,"warning":"","streams":[{"codec_type":"video","codec_name":"h264","width":width,"height":height,"avg_frame_rate":"24/1"}]}).to_string()
    };
    for e in &episodes {
        let (width, height) = if e.title == "第一集" {
            (3840, 2160)
        } else if e.line_name == "lzm3u8" {
            (1280, 720)
        } else {
            (640, 360)
        };
        sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
            .bind(cache(width, height))
            .bind(now())
            .bind(&e.id)
            .execute(&h.app.db)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(cache(640, 360))
        .bind(now())
        .bind(&current.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    let path = format!(
        "/api/films/{}/source-quality?switch_from={}&switch_episode={}",
        target.id, original.id, current.id
    );
    let hits = h.hits.load(Ordering::Relaxed);
    let (status, result) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(status, 200);
    assert_eq!(result["status"], "measured");
    assert_eq!(
        result["quality"]["height"].as_f64(),
        Some(720.0),
        "不能把第一集 4K 或上游蓝光标签作为第三集的画质"
    );
    assert_eq!(result["episode_title"], "第三集");
    assert_eq!(result["line_name"], "lzm3u8");
    assert_eq!(result["lines"].as_array().unwrap().len(), 2);
    assert!(
        !result.to_string().contains("https://"),
        "画质接口只返回测量结果，不交付媒体地址"
    );
    request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "有效探测缓存不重复请求媒体"
    );
    let (_, own) = request(
        &h,
        "GET",
        &format!(
            "/api/films/{}/source-quality?switch_from={}&switch_episode={}",
            original.id, original.id, current.id
        ),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(own["quality"]["height"].as_f64(), Some(360.0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM plays")
            .fetch_one(&h.app.db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM progress")
            .fetch_one(&h.app.db)
            .await
            .unwrap(),
        0
    );
    let unknown =
        json!({"kind":"video","duration":0,"size":0,"warning":"","streams":[]}).to_string();
    for e in episodes {
        sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
            .bind(&unknown)
            .bind(now())
            .bind(&e.id)
            .execute(&h.app.db)
            .await
            .unwrap();
    }
    let (_, result) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["quality"]["height"].as_f64(), Some(0.0));
}

#[tokio::test]
async fn source_quality_rejects_unrelated_or_ambiguous_episodes_before_media_requests() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let original = catalog::ingest(&h.app, &source, &json!({"vod_id":811,"vod_name":"画质校验剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"原线路","vod_play_url":"第03集$https://example.com/3.mp4"}), true).await.unwrap().unwrap();
    let episode = catalog::episodes(&h.app, &original.id)
        .await
        .unwrap()
        .remove(0);
    let ambiguous = catalog::ingest(&h.app, &source, &json!({"vod_id":812,"vod_name":"画质校验剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"重复集号$$$合集","vod_play_url":"第03集$https://example.com/a.mp4#第003集$https://example.com/b.mp4$$$第01-03集$https://example.com/c.mp4"}), true).await.unwrap().unwrap();
    let wrong_year = catalog::ingest(&h.app, &source, &json!({"vod_id":813,"vod_name":"画质校验剧集","vod_year":"2025","type_name":"电视剧","vod_play_from":"原线路","vod_play_url":"第03集$https://example.com/3.mp4"}), true).await.unwrap().unwrap();
    let path = |target: &str, ep: &str| {
        format!(
            "/api/films/{target}/source-quality?switch_from={}&switch_episode={ep}",
            original.id
        )
    };
    let hits = h.hits.load(Ordering::Relaxed);
    let (status, result) = request(
        &h,
        "GET",
        &path(&ambiguous.id, &episode.id),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(result["status"], "episode_required");
    assert!(result["lines"].as_array().unwrap().is_empty());
    assert_eq!(
        request(
            &h,
            "GET",
            &path(&wrong_year.id, &episode.id),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            &h,
            "GET",
            &path(&ambiguous.id, &h.episode),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            &h,
            "GET",
            &path(&ambiguous.id, &episode.id),
            json!({}),
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
}

#[tokio::test]
async fn source_quality_refreshes_updated_media_instead_of_reusing_old_dimensions() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let api = url::Url::parse(&source.api)
        .unwrap()
        .join("/quality-api")
        .unwrap();
    sqlx::query("UPDATE sources SET api=? WHERE id=?")
        .bind(api.as_str())
        .bind(&source.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(json!({"kind":"mp4","duration":120,"size":8,"warning":"","streams":[{"codec_type":"video","width":1920,"height":1080}]}).to_string())
        .bind(now()).bind(&h.episode).execute(&h.app.db).await.unwrap();
    let path = format!(
        "/api/films/{}/source-quality?switch_from={}&switch_episode={}",
        h.film, h.film, h.episode
    );
    let hits = h.hits.load(Ordering::Relaxed);
    let (_, cached) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(cached["quality"]["width"].as_f64(), Some(1920.0));
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
    let (status, fresh) = request(
        &h,
        "GET",
        &format!("{path}&refresh=1"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(fresh["status"], "measured");
    assert_eq!(
        fresh["quality"]["width"].as_f64(),
        Some(64.0),
        "重新获取当前来源的更新地址并实际探测加密 HLS"
    );
    assert_eq!(fresh["quality"]["codec"], "h264");
    assert!(h.hits.load(Ordering::Relaxed) > hits);
    assert!(
        catalog::episode(&h.app, &h.episode)
            .await
            .unwrap()
            .url
            .ends_with("/encrypted.m3u8")
    );
    let hits = h.hits.load(Ordering::Relaxed);
    let (_, cached) = request(&h, "GET", &path, json!({}), Some(&h.member_raw)).await;
    assert_eq!(cached["quality"], fresh["quality"]);
    assert_eq!(h.hits.load(Ordering::Relaxed), hits);
    sqlx::query("UPDATE episodes SET probe='{}',probed_at=0 WHERE id=?")
        .bind(&h.episode)
        .execute(&h.app.db)
        .await
        .unwrap();
    let background_slot = h.app.source_quality_slots.acquire().await.unwrap();
    let (status, selected) = tokio::time::timeout(
        Duration::from_secs(3),
        request(
            &h,
            "GET",
            &format!("{path}&selected=1"),
            json!({}),
            Some(&h.member_raw),
        ),
    )
    .await
    .expect("用户点击的来源确认不应排在后台探测之后");
    assert_eq!(status, 200);
    assert_eq!(selected["quality"], fresh["quality"]);
    drop(background_slot);
}

#[tokio::test]
async fn background_source_quality_leaves_playback_capacity_and_rechecks_revoked_access() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let subscription = catalog::import(&h.app,&json!({"name":"画质授权订阅","content":json!({"api_site":{"quality":{"name":"画质源","api":source.api}}}).to_string()})).await.unwrap();
    let target_source = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.subscription_id == subscription)
        .unwrap();
    let media = url::Url::parse(&source.api)
        .unwrap()
        .join("/slow")
        .unwrap()
        .to_string();
    let target = catalog::ingest(&h.app,&target_source,&json!({"vod_id":821,"vod_name":"测试电影","vod_year":"2026","vod_play_from":"线路","vod_play_url":format!("正片${media}")}),true).await.unwrap().unwrap();
    let background_slot = h.app.source_quality_slots.acquire().await.unwrap();
    let task_app = h.app.clone();
    let member = h.other.user.id.clone();
    let target_id = target.id.clone();
    let from = h.film.clone();
    let reference = h.episode.clone();
    let background = tokio::spawn(async move {
        catalog::source_quality(
            &task_app, &member, &target_id, &from, &reference, false, false,
        )
        .await
    });
    let started = tokio::time::timeout(
        Duration::from_secs(3),
        playback::start(&h.app, &h.who, &h.episode),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!background.is_finished(), "后台排队不阻塞真实播放的探测");
    assert!(!text(&started, "id").is_empty());
    let hits = h.hits.load(Ordering::Relaxed);
    access::save_usage(
        &h.app,
        &subscription,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    drop(background_slot);
    let error = tokio::time::timeout(Duration::from_secs(3), background)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.0, StatusCode::FORBIDDEN);
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "排队期间撤权的来源不得发出媒体请求"
    );
    assert_eq!(
        request(
            &h,
            "GET",
            &format!(
                "/api/films/{}/source-quality?switch_from={}&switch_episode={}",
                target.id, h.film, h.episode
            ),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
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
    sqlx::query("WITH RECURSIVE items(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM items WHERE n<11000) INSERT INTO media_assets(id,play_id,url) SELECT 'bounded-'||n,?,'https://media.example/existing' FROM items")
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
    sqlx::query("WITH RECURSIVE items(n) AS (VALUES(11001) UNION ALL SELECT n+1 FROM items WHERE n<11998) INSERT INTO media_assets(id,play_id,url) SELECT 'bounded-'||n,?,'https://media.example/existing' FROM items")
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
    assert_eq!(info.probe_version, 2);
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
    assert_eq!(v["data"][0]["title"], "原生搜索结果");
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

#[tokio::test]
async fn native_continue_watching_groups_works_and_deletes_all_sources_for_only_its_user() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let at = now();
    let mut films = Vec::new();
    for index in 0..3 {
        films.push(watched_film(&h,&source,json!({"vod_id":6000+index,"vod_name":"原生继续观看","vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":"第01集$https://example.com/1.mp4#第03集$https://example.com/3.mp4"}),
            (&h.other.user.id,[53.0,12.0,2.0][index as usize],index,at)).await);
    }
    for film in &films {
        let episode = catalog::episodes(&h.app, &film.id).await.unwrap().remove(1);
        sqlx::query("UPDATE progress SET episode_id=? WHERE user_id=? AND film_id=?")
            .bind(&episode.id)
            .bind(&h.other.user.id)
            .bind(&film.id)
            .execute(&h.app.db)
            .await
            .unwrap();
    }
    let different = watched_film(&h,&source,json!({"vod_id":6010,"vod_name":"原生继续观看","vod_year":"2025","type_name":"电视剧","vod_play_from":"line","vod_play_url":"第03集$https://example.com/3.mp4"}),
        (&h.other.user.id,9.0,0,at-1)).await;
    for index in 0..7 {
        watched_film(&h,&source,json!({"vod_id":6020+index,"vod_name":format!("原生其他作品{index}"),"vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":"第03集$https://example.com/3.mp4"}),
            (&h.other.user.id,3.0,0,at-2-index)).await;
    }
    catalog::favorite(&h.app, &h.other.user.id, &films[0].id, true)
        .await
        .unwrap();
    let (_, rows) = request(
        &h,
        "GET",
        "/v/api/v1/play/list",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    let items = rows["data"].as_array().unwrap();
    assert_eq!(items.len(), 9, "原生列表不能套用首页六条的限制");
    assert_ne!(items[0]["grand_guid"], films[2].id);
    assert_eq!(items[0]["tv_title"], "原生继续观看");
    let (_, library) = request(
        &h,
        "POST",
        "/v/api/v1/item/list",
        json!({"ancestor_guid":tv_rust::fntv::library("TV")}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(library["data"]["total"], 9);
    let shows: Vec<_> = library["data"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["title"] == "原生继续观看")
        .collect();
    assert_eq!(
        shows.len(),
        2,
        "只有不同年份可分别显示，同年三个来源合为一条"
    );
    assert!(
        shows
            .iter()
            .any(|show| show["guid"] == items[0]["grand_guid"])
    );
    assert_eq!(items[0]["ts"], 2);
    assert_eq!(
        items[0]["episode_number"], 3,
        "真实集号不能采用目录中的第二项"
    );
    let latest = text(&items[0], "guid");
    let (status, grouped) =
        request(&h, "GET", "/api/history", json!({}), Some(&h.member_raw)).await;
    assert_eq!(status, 200);
    assert_eq!(grouped["items"].as_array().unwrap().len(), 9);
    assert_eq!(grouped["items"][0]["id"], films[2].id);
    assert_eq!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .len(),
        11
    );
    assert_eq!(
        request(&h, "GET", "/v/api/v1/play/list", json!({}), Some(&h.raw))
            .await
            .1["data"],
        json!([])
    );
    let own_episode = catalog::episode(&h.app, &text(&items[0], "media_guid"))
        .await
        .unwrap();
    sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?)")
        .bind(&h.who.user.id)
        .bind(&films[2].id)
        .bind(&own_episode.id)
        .bind(22.0)
        .bind(120.0)
        .bind(0)
        .bind(at)
        .execute(&h.app.db)
        .await
        .unwrap();
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    let hits = h.hits.load(Ordering::Relaxed);
    assert_eq!(
        request(
            &h,
            "DELETE",
            "/v/api/v1/play/record",
            json!({"item_guid":latest}),
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(
        request(
            &h,
            "DELETE",
            "/v/api/v1/play/record",
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        400
    );
    let (status, removed) = request(
        &h,
        "DELETE",
        "/v/api/v1/play/record",
        json!({"item_guid":latest}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(removed["data"], true);
    assert_eq!(
        h.hits.load(Ordering::Relaxed),
        hits,
        "删除已失效的个人记录无需请求媒体"
    );
    let history = catalog::personal(&h.app, &h.other.user.id, "history")
        .await
        .unwrap();
    assert_eq!(history.len(), 8);
    assert!(history.iter().any(|item| item["id"] == different.id));
    assert!(
        films
            .iter()
            .all(|film| !history.iter().any(|item| item["id"] == film.id))
    );
    assert_eq!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()[0]["progress"]["position"],
        22.0
    );
    assert_eq!(
        catalog::personal(&h.app, &h.other.user.id, "favorites")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        request(
            &h,
            "DELETE",
            "/v/api/v1/play/record",
            json!({"item_guid":latest}),
            Some(&h.member_raw)
        )
        .await
        .0,
        200
    );
}

async fn cached_native_media(h: &Harness, episode: &str, width: u64, height: u64) {
    let streams = if width == 0 {
        vec![]
    } else {
        vec![
            json!({"index":0,"codec_type":"video","codec_name":"h264","width":width,"height":height,"avg_frame_rate":"24/1","bit_rate":"4200000"}),
        ]
    };
    sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
        .bind(json!({"probe_version":1,"kind":"mp4","duration":120.0,"size":8,"streams":streams,"warning":""}).to_string())
        .bind(now()).bind(episode).execute(&h.app.db).await.unwrap();
}

#[tokio::test]
async fn native_source_versions_match_the_actual_episode_sort_quality_and_keep_source_progress() {
    let h = harness().await;
    let primary = catalog::sources(&h.app).await.unwrap().remove(0);
    let subscription = catalog::import(&h.app,&json!({"name":"原生候选订阅","content":json!({"api_site":{"b":{"name":"🎬红牛资源","api":primary.api},"c":{"name":"🎬金鹰点播","api":primary.api}}}).to_string()})).await.unwrap();
    let sources: Vec<_> = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .filter(|source| source.subscription_id == subscription)
        .collect();
    let secondary = sources
        .iter()
        .find(|source| source.name == "🎬红牛资源")
        .unwrap();
    let unknown = sources
        .iter()
        .find(|source| source.name == "🎬金鹰点播")
        .unwrap();
    let reference = catalog::episode(&h.app, &h.episode).await.unwrap();
    let url = reference.url;
    let a = catalog::ingest(&h.app,&primary,&json!({"vod_id":6101,"vod_name":"原生换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"liangzi$$$lzm3u8","vod_play_url":format!("第01集${url}#第03集${url}$$$第01集${url}#第03集${url}")}),true).await.unwrap().unwrap();
    let b = catalog::ingest(&h.app,secondary,&json!({"vod_id":6102,"vod_name":"原生换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"liangzi$$$lzm3u8","vod_play_url":format!("第一集${url}#第二集${url}#第三集${url}$$$第一集${url}#第二集${url}#第三集${url}")}),true).await.unwrap().unwrap();
    let c = catalog::ingest(&h.app,unknown,&json!({"vod_id":6103,"vod_name":"原生换源剧集","vod_year":"2026","type_name":"电视剧","vod_remarks":"4K蓝光","vod_play_from":"lzm3u8","vod_play_url":format!("第三集${url}")}),true).await.unwrap().unwrap();
    let wrong = catalog::ingest(&h.app,secondary,&json!({"vod_id":6104,"vod_name":"原生换源剧集","vod_year":"2025","type_name":"电视剧","vod_play_from":"line","vod_play_url":format!("第三集${url}")}),true).await.unwrap().unwrap();
    let ambiguous = catalog::ingest(&h.app,unknown,&json!({"vod_id":6105,"vod_name":"原生换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":format!("第01-03集${url}")}),true).await.unwrap().unwrap();
    let hidden_subscription = catalog::import(&h.app,&json!({"name":"原生隐藏订阅","usage":{"mode":"admins","user_ids":[]},"content":json!({"api_site":{"hidden":{"name":"原生隐藏源","api":primary.api}}}).to_string()})).await.unwrap();
    let hidden = catalog::sources(&h.app)
        .await
        .unwrap()
        .into_iter()
        .find(|source| source.subscription_id == hidden_subscription)
        .unwrap();
    catalog::ingest(&h.app,&hidden,&json!({"vod_id":6106,"vod_name":"原生换源剧集","vod_year":"2026","type_name":"电视剧","vod_play_from":"line","vod_play_url":format!("第三集${url}")}),true).await.unwrap();
    let a_eps = catalog::episodes(&h.app, &a.id).await.unwrap();
    let b_eps = catalog::episodes(&h.app, &b.id).await.unwrap();
    for e in &a_eps {
        cached_native_media(
            &h,
            &e.id,
            if e.line_order == 0 { 640 } else { 1920 },
            if e.line_order == 0 { 360 } else { 1080 },
        )
        .await;
    }
    for e in &b_eps {
        cached_native_media(
            &h,
            &e.id,
            if e.line_order == 0 { 640 } else { 1280 },
            if e.line_order == 0 { 360 } else { 720 },
        )
        .await;
    }
    let c_ep = catalog::episodes(&h.app, &c.id).await.unwrap().remove(0);
    cached_native_media(&h, &c_ep.id, 0, 0).await;
    let a_third = a_eps
        .iter()
        .find(|e| e.title == "第03集" && e.line_order == 0)
        .unwrap();
    let b_third = b_eps
        .iter()
        .find(|e| e.title == "第三集" && e.line_order == 1)
        .unwrap();
    for (film, e, position) in [(&a, a_third, 12.0), (&b, b_third, 7.0)] {
        sqlx::query("INSERT INTO progress VALUES(?,?,?,?,?,?,?)")
            .bind(&h.other.user.id)
            .bind(&film.id)
            .bind(&e.id)
            .bind(position)
            .bind(120.0)
            .bind(0)
            .bind(now())
            .execute(&h.app.db)
            .await
            .unwrap();
    }
    let (status, versions) = request(
        &h,
        "GET",
        &format!("/v/api/v1/stream/list/{}", a_third.id),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        versions["data"]["files"].as_array().unwrap().len(),
        1,
        "详情只展示当前媒体，来源属于播放菜单"
    );
    let menu = versions["data"]["qualities"].as_array().unwrap();
    assert_eq!(menu.len(), 5);
    assert_eq!(menu[0]["resolution"], format!("{}/2", primary.name));
    assert_eq!(menu[1]["resolution"], "红牛资源/2");
    assert_eq!(menu.last().unwrap()["resolution"], "金鹰点播");
    assert!(menu.iter().all(|choice| {
        let label = text(choice, "resolution");
        !label.contains('🎬') && !label.contains("画质") && !label.contains('·')
    }));
    assert_eq!(
        versions["data"]["files"][0]["quality"]["width"].as_f64(),
        Some(640.0)
    );
    assert_eq!(
        versions["data"]["files"][0]["quality"]["height"].as_f64(),
        Some(360.0)
    );
    assert!(!versions.to_string().contains("原生隐藏源"));
    assert!(!versions.to_string().contains(&url));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM plays")
            .fetch_one(&h.app.db)
            .await
            .unwrap(),
        0
    );
    let (_, seasons) = request(
        &h,
        "GET",
        &format!("/v/api/v1/season/list/{}", a.id),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    let seasons = seasons["data"].as_array().unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0]["title"], "第 1 季");
    assert_eq!(
        seasons[0]["number_of_episodes"], 3,
        "补齐其他来源确认为同作品的第二集"
    );
    let season = text(&seasons[0], "guid");
    let (_, episodes) = request(
        &h,
        "GET",
        &format!("/v/api/v1/episode/list/{season}"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    let eps = episodes["data"].as_array().unwrap();
    assert_eq!(eps.len(), 3);
    assert_eq!(eps[2]["episode_number"], 3);
    let (_, native_directory) = request(
        &h,
        "POST",
        "/v/api/v1/item/list",
        json!({"ancestor_guid":season}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(
        native_directory["data"]["list"].as_array().unwrap().len(),
        3
    );
    assert_eq!(native_directory["data"]["list"][1]["episode_number"], 2);
    let canonical = text(&eps[2], "guid");
    let (_, first) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":a_third.id}),
        Some(&h.member_raw),
    )
    .await;
    let lines = first["data"]["direct_link_qualities"].as_array().unwrap();
    assert_eq!(lines.len(), 5);
    assert!(
        lines
            .iter()
            .all(|line| line["progressive"] == true && line["is_m3u8"] == false)
    );
    let selected = lines
        .iter()
        .find(|line| text(line, "resolution") == "红牛资源/2")
        .unwrap();
    let selected_link = text(selected, "url");
    let current_index = first["data"]["direct_link_quality_index"].as_u64().unwrap() as usize;
    let first_link = text(&lines[current_index], "url");
    assert_eq!(
        first["data"]["video_stream"]["resolution_type"], lines[current_index]["resolution"],
        "起播选择当前来源，不能误选画质排序的第一项"
    );
    assert_eq!(
        first["data"]["video_stream"]["bps"],
        lines[current_index]["bitrate"]
    );
    assert_ne!(
        first["data"]["quality"]["bitrate"], lines[current_index]["bitrate"],
        "菜单选择码不能用作实测码率"
    );
    assert_eq!(first["data"]["header"], json!({}));
    let (status, play) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":a_third.id,"item_guid":canonical,"bitrate":selected["bitrate"]}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(play["data"]["media_guid"], b_third.id);
    assert_eq!(play["data"]["play_link"], selected_link);
    let (_, info) = request(
        &h,
        "POST",
        "/v/api/v1/play/info",
        json!({"item_guid":canonical}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(info["data"]["media_guid"], b_third.id);
    assert_eq!(info["data"]["ts"], 7, "跨源读取目标自己的进度");
    let selected_url = url::Url::parse(&selected_link).unwrap();
    let response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(selected_url.path())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(response.headers()["location"], url, "源地址与签名不被改写");
    let record =
        json!({"item_guid":canonical,"media_guid":a_third.id,"play_link":selected_link,"ts":10});
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/record",
            record,
            Some(&h.member_raw)
        )
        .await
        .0,
        200
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":a_third.id,"play_link":first_link,"ts":99}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.other.user.id, &a.id, false)
            .await
            .unwrap()["progress"]["position"],
        12.0,
        "旧源迟到回传不能覆盖切换结果"
    );
    assert_eq!(
        catalog::view(&h.app, &h.other.user.id, &b.id, false)
            .await
            .unwrap()["progress"]["position"],
        10.0
    );
    let (_, latest) = request(
        &h,
        "GET",
        "/v/api/v1/play/list",
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    assert_eq!(latest["data"].as_array().unwrap().len(), 1);
    assert_eq!(latest["data"][0]["media_guid"], b_third.id);
    let b_second = b_eps.iter().find(|e| e.title == "第二集").unwrap();
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/info",
            json!({"item_guid":canonical,"media_guid":b_second.id}),
            Some(&h.member_raw)
        )
        .await
        .0,
        400
    );
    assert_eq!(request(&h,"POST","/v/api/v1/play/info",json!({"item_guid":canonical,"media_guid":catalog::episodes(&h.app,&wrong.id).await.unwrap()[0].id}),Some(&h.member_raw)).await.0,400);
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/play",
            json!({"media_guid":a_third.id,"bitrate":99999999}),
            Some(&h.member_raw)
        )
        .await
        .0,
        400
    );
    playback::remove_history(&h.app, &h.other.user.id, std::slice::from_ref(&a.id))
        .await
        .unwrap();
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":b_third.id,"play_link":selected_link,"ts":25}),
        Some(&h.member_raw),
    )
    .await;
    assert!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    let (_, other_plays) = request(&h, "GET", "/v/api/v1/play/list", json!({}), Some(&h.raw)).await;
    assert_eq!(other_plays["data"], json!([]));
    access::save_usage(
        &h.app,
        &subscription,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    assert_eq!(
        request(&h, "GET", selected_url.path(), json!({}), None)
            .await
            .0,
        403,
        "已签发的菜单也检查最新权限"
    );
    let _ = (c_ep, ambiguous);
}

#[tokio::test]
async fn deleting_history_blocks_old_web_native_and_final_reports_but_allows_new_playback() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let original = catalog::episode(&h.app, &h.episode).await.unwrap();
    let alternate=catalog::ingest(&h.app,&source,&json!({"vod_id":6200,"vod_name":"测试电影","vod_year":"2026","vod_play_from":"line","vod_play_url":format!("正片${}",original.url)}),true).await.unwrap().unwrap();
    let alternate_ep = catalog::episodes(&h.app, &alternate.id)
        .await
        .unwrap()
        .remove(0);
    cached_native_media(&h, &alternate_ep.id, 1280, 720).await;
    let web = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&web, "id")).await.unwrap();
    playback::record(&h.app, &p, 12.0, 120.0, None)
        .await
        .unwrap();
    catalog::favorite(&h.app, &h.who.user.id, &h.film, true)
        .await
        .unwrap();
    let (native_token, _) = auth::login(&h.app, "admin", &hash("password-123"), true)
        .await
        .unwrap();
    let (_, native) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":alternate_ep.id}),
        Some(&native_token),
    )
    .await;
    let link = text(&native["data"]["direct_link_qualities"][0], "url");
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":alternate_ep.id,"play_link":link,"ts":7}),
        Some(&native_token),
    )
    .await;
    let other = playback::start(&h.app, &h.other, &h.episode).await.unwrap();
    let other_p = playback::get(&h.app, &text(&other, "id")).await.unwrap();
    playback::record(&h.app, &other_p, 43.0, 120.0, None)
        .await
        .unwrap();
    assert_eq!(
        request(
            &h,
            "DELETE",
            &format!("/api/films/{}/history", h.film),
            json!({}),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    assert!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    let updated = playback::get(&h.app, &p.id).await.unwrap();
    assert!(updated.recording_disabled);
    assert!(!updated.ended, "删除记录不截断正在播放的媒体");
    assert!(
        !playback::record(&h.app, &p, 99.0, 120.0, None)
            .await
            .unwrap()
    );
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/record",
            json!({"media_guid":alternate_ep.id,"play_link":link,"ts":99}),
            Some(&native_token)
        )
        .await
        .0,
        200
    );
    playback::end_with_snapshot(&h.app, &p, Some((100.0, 120.0)))
        .await
        .unwrap();
    assert!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        catalog::view(&h.app, &h.other.user.id, &h.film, false)
            .await
            .unwrap()["progress"]["position"],
        43.0
    );
    assert_eq!(
        catalog::personal(&h.app, &h.who.user.id, "favorites")
            .await
            .unwrap()
            .len(),
        1
    );
    let (_, fresh) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":alternate_ep.id}),
        Some(&native_token),
    )
    .await;
    let new_link = text(&fresh["data"]["direct_link_qualities"][0], "url");
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":alternate_ep.id,"play_link":new_link,"ts":3}),
        Some(&native_token),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &alternate.id, false)
            .await
            .unwrap()["progress"]["position"],
        3.0
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":alternate_ep.id,"play_link":link,"ts":99}),
        Some(&native_token),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &alternate.id, false)
            .await
            .unwrap()["progress"]["position"],
        3.0
    );
}

#[tokio::test]
async fn deleting_history_is_atomic_with_inflight_reports_and_rolls_back_on_failure() {
    let h = harness().await;
    let started = playback::start(&h.app, &h.who, &h.episode).await.unwrap();
    let p = playback::get(&h.app, &text(&started, "id")).await.unwrap();
    playback::record(&h.app, &p, 12.0, 120.0, None)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_history_deletion BEFORE DELETE ON progress BEGIN SELECT RAISE(ABORT,'controlled failure'); END;").execute(&h.app.db).await.unwrap();
    assert!(
        playback::remove_history(&h.app, &h.who.user.id, std::slice::from_ref(&h.film))
            .await
            .is_err()
    );
    assert!(
        !playback::get(&h.app, &p.id)
            .await
            .unwrap()
            .recording_disabled
    );
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &h.film, false)
            .await
            .unwrap()["progress"]["position"],
        12.0
    );
    sqlx::raw_sql("DROP TRIGGER reject_history_deletion;")
        .execute(&h.app.db)
        .await
        .unwrap();
    let (removed, late, ended) = tokio::join!(
        playback::remove_history(&h.app, &h.who.user.id, std::slice::from_ref(&h.film)),
        playback::record(&h.app, &p, 55.0, 120.0, None),
        playback::end_with_snapshot(&h.app, &p, Some((77.0, 120.0)))
    );
    removed.unwrap();
    late.unwrap();
    ended.unwrap();
    assert!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        playback::get(&h.app, &p.id)
            .await
            .unwrap()
            .recording_disabled
    );
}

#[tokio::test]
async fn native_work_and_episode_identities_survive_restart_and_recheck_access() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let url = catalog::episode(&h.app, &h.episode).await.unwrap().url;
    let film=catalog::ingest(&h.app,&source,&json!({"vod_id":6300,"vod_name":"统一作品验收","vod_year":"2026","type_name":"电视剧","vod_content":"单一作品简介","vod_pic":"https://example.com/poster.jpg","vod_play_from":"lzm3u8","vod_play_url":"第03集$".to_owned()+&url}),true).await.unwrap().unwrap();
    let episode = catalog::episodes(&h.app, &film.id).await.unwrap().remove(0);
    cached_native_media(&h, &episode.id, 1280, 720).await;
    let (_, seasons) = request(
        &h,
        "GET",
        &format!("/v/api/v1/season/list/{}", film.id),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    let season = text(&seasons["data"][0], "guid");
    assert_eq!(seasons["data"][0]["title"], "第 1 季");
    assert_eq!(seasons["data"][0]["overview"], "单一作品简介");
    assert!(text(&seasons["data"][0], "poster").starts_with("/tv-poster/"));
    let (_, eps) = request(
        &h,
        "GET",
        &format!("/v/api/v1/episode/list/{season}"),
        json!({}),
        Some(&h.member_raw),
    )
    .await;
    let canonical = text(&eps["data"][0], "guid");
    assert_ne!(canonical, episode.id);
    sqlx::query("DELETE FROM cache")
        .execute(&h.app.db)
        .await
        .unwrap();
    let restarted = App::new(h._dir.path().into(), true, String::new())
        .await
        .unwrap();
    for guid in [&season, &canonical] {
        let response = router(restarted.clone(), "/nonexistent")
            .oneshot(
                Request::builder()
                    .uri(format!("/v/api/v1/item/{guid}"))
                    .header("authorization", format!("Bearer {}", h.member_raw))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "作品与单集身份不依赖搜索缓存或进程生命周期"
        );
    }
    access::save_usage(
        &h.app,
        &source.subscription_id,
        &json!({"mode":"admins","user_ids":[]}),
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &h,
            "GET",
            &format!("/v/api/v1/item/{canonical}"),
            json!({}),
            Some(&h.member_raw)
        )
        .await
        .0,
        403
    );
    sqlx::query("UPDATE episodes SET active=0 WHERE id=?")
        .bind(&episode.id)
        .execute(&h.app.db)
        .await
        .unwrap();
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/info",
            json!({"item_guid":canonical}),
            Some(&h.raw)
        )
        .await
        .0,
        400,
        "参考单集失效后不能静默播放其他集"
    );
}

async fn native_hls_fixture(h: &Harness, extensionless: bool) -> Vec<catalog::Episode> {
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let base = url::Url::parse(&catalog::episode(&h.app, &h.episode).await.unwrap().url).unwrap();
    let first = base
        .join(if extensionless {
            "/master"
        } else {
            "/encrypted.m3u8"
        })
        .unwrap();
    let second = base.join("/encrypted.m3u8?variant=2").unwrap();
    let film = catalog::ingest(&h.app, &source, &json!({
        "vod_id":7100,"vod_name":"原生HLS交付验收","vod_year":"2026","type_name":"电影",
        "vod_play_from":"lzm3u8$$$备用线路","vod_play_url":format!("正片${first}$$$正片${second}")
    }), true).await.unwrap().unwrap();
    let eps = catalog::episodes(&h.app, &film.id).await.unwrap();
    for e in &eps {
        sqlx::query("UPDATE episodes SET probe=?,probed_at=? WHERE id=?")
            .bind(json!({"probe_version":1,"kind":"hls","duration":120.0,"size":0,"streams":[
                {"index":0,"codec_type":"video","codec_name":"h264","width":1280,"height":720,"avg_frame_rate":"24/1","bit_rate":"4200000"}
            ],"warning":""}).to_string()).bind(now()).bind(&e.id).execute(&h.app.db).await.unwrap();
    }
    catalog::episodes(&h.app, &film.id).await.unwrap()
}

#[tokio::test]
async fn native_hls_uses_a_ready_manifest_instead_of_the_strm_file_channel() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    assert!(
        stream["data"]["cloud_storage_info"].is_null(),
        "HLS不能作为网盘原文件直链交付"
    );
    assert!(
        stream["data"]["direct_link_qualities"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    let qualities = stream["data"]["qualities"].as_array().unwrap();
    assert_eq!(qualities.len(), 2);
    assert!(
        qualities
            .iter()
            .all(|quality| quality["progressive"] == true && quality["is_m3u8"] == false)
    );
    assert!(
        qualities
            .iter()
            .any(|quality| text(quality, "resolution") == "测试源/1")
    );
    let (_, played) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(played["data"]["wrapper"], "HLS");
    let play_link = text(&played["data"], "play_link");
    assert!(
        play_link.starts_with("/v/native-menu/"),
        "原生HLS换源交付同站相对清单地址"
    );
    let link = url::Url::parse("http://127.0.0.1:3000")
        .unwrap()
        .join(&play_link)
        .unwrap();
    assert!(
        link.path().ends_with(".m3u8"),
        "原生HLS媒体地址必须保留格式后缀"
    );
    let response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(link.path())
                .header("range", "bytes=0-0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "入口直接交付HLS主清单，不能仅假设客户端跟随无后缀跳转"
    );
    assert_eq!(
        response.headers()["content-type"],
        "application/vnd.apple.mpegurl"
    );
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.starts_with("#EXTM3U"));
    assert!(text.contains("/media/"));
    assert!(!text.contains(&eps[0].url));
    assert_eq!(stream["data"]["playback_mode"], "proxy");
    assert!(
        stream["data"]["file_stream"]["file_name"]
            .as_str()
            .unwrap()
            .starts_with("转发 · ")
    );
}

#[tokio::test]
async fn native_hls_forbidden_proxy_uses_strm_without_media_authentication_headers() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    playback::set_forbidden(&h.app, true).await.unwrap();
    let (_, pending) = request(
        &h,
        "GET",
        &format!("/v/api/v1/stream/list/{}", eps[0].id),
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(pending["data"]["playback_mode_pending"], true);
    assert_eq!(pending["data"]["playback_mode_label"], "直链（待播放）");
    let (status, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        stream["data"]["cloud_storage_info"]["cloud_storage_type"],
        9001
    );
    assert_eq!(stream["data"]["header"], json!({}));
    assert_eq!(stream["data"]["playback_mode"], "direct");
    assert_eq!(stream["data"]["playback_mode_pending"], false);
    assert!(text(&stream["data"]["file_stream"], "file_name").starts_with("直链 · "));
    assert!(text(&stream["data"]["file_stream"], "path").starts_with("/播放方式/直链/"));
    assert!(text(&stream["data"]["video_stream"], "title").starts_with("直链 · "));
    let choices = stream["data"]["direct_link_qualities"].as_array().unwrap();
    assert_eq!(choices.len(), 2);
    assert!(choices.iter().all(|choice| choice["is_m3u8"] == true));
    let selected = stream["data"]["direct_link_quality_index"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(selected, 0, "STRM 首次读取必须使用本次选择的源");
    assert_eq!(
        choices[0]["bitrate"], stream["data"]["video_stream"]["bps"],
        "来源标签、选择码和首次媒体读取必须对应同一个来源"
    );
    let link = text(&choices[selected], "url");
    let path = url::Url::parse(&link).unwrap().path().to_owned();
    assert!(path.ends_with(".m3u8"));
    let hits = h.hits.load(Ordering::Relaxed);
    for method in ["GET", "HEAD"] {
        let response = router(h.app.clone(), "/nonexistent")
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("range", "bytes=0-0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "正常 HLS 读取不能误报转发被禁");
        assert_eq!(
            response.headers()["content-type"],
            "application/vnd.apple.mpegurl"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        if method == "GET" {
            let manifest = std::str::from_utf8(&bytes).unwrap();
            assert!(manifest.starts_with("#EXTM3U\n#EXT-X-STREAM-INF:"));
            assert!(manifest.contains(&eps[0].url));
            assert!(!manifest.contains("/media/"));
            assert!(manifest.contains("BANDWIDTH=4200000"));
        } else {
            assert!(bytes.is_empty());
        }
    }
    assert_eq!(h.hits.load(Ordering::Relaxed), hits, "直链清单不转发源资源");
    let play = playback::latest(&h.app, &h.who, &eps[0].id).await.unwrap();
    assert_eq!(play.mode, "direct");
    let (_, recorded) = request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":eps[0].id,"play_link":link,"ts":15,
            "bitrate":stream["data"]["video_stream"]["bps"]}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(recorded["data"], true);
    assert_eq!(
        playback::get(&h.app, &play.id).await.unwrap().position,
        15.0
    );
    assert!(
        playback::fallback(&h.app, &h.who, &play.id, "manual")
            .await
            .is_err()
    );
    assert_eq!(
        playback::get(&h.app, &play.id).await.unwrap().mode,
        "direct"
    );
    for method in ["GET", "HEAD"] {
        assert_eq!(
            request(&h, method, &path, json!({}), Some(&h.raw)).await.0,
            403
        );
        assert_eq!(
            request(
                &h,
                method,
                &format!("/v/api/v1/media/range/{}", eps[0].id),
                json!({}),
                Some(&h.raw)
            )
            .await
            .0,
            403
        );
    }
    let (_, files) = request(
        &h,
        "GET",
        &format!("/v/api/v1/stream/list/{}", eps[0].id),
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(files["data"]["playback_mode"], "direct");
    assert!(text(&files["data"]["files"][0], "file_name").starts_with("直链 · "));

    let (status, switched) = request(
        &h,
        "POST",
        "/v/api/v1/stream/directLink",
        json!({"media_guid":eps[0].id,"direct_link_quality_index":1}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(switched["data"]["file_stream"]["guid"], eps[1].id);
    assert_eq!(
        switched["data"]["video_stream"]["bps"],
        choices[1]["bitrate"]
    );
    assert_eq!(switched["data"]["direct_link_quality_index"], 1);
    let (status, restored) = request(
        &h,
        "POST",
        "/v/api/v1/stream/directLink",
        json!({"media_guid":eps[1].id,"direct_link_quality_index":0}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(restored["data"]["file_stream"]["guid"], eps[0].id);
    assert_eq!(
        restored["data"]["video_stream"]["bps"],
        choices[0]["bitrate"]
    );
}

#[tokio::test]
async fn native_hls_binds_the_sdk_link_before_the_first_authenticated_media_read() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let sdk = "12345678-1234-1234-1234-123456789abc";
    let path = format!("/v/api/v1/media/range/{}?playlink={sdk}", eps[0].id);
    assert_eq!(
        request(&h, "GET", &path, json!({}), Some(&h.raw)).await.0,
        200
    );
    let play = playback::latest(&h.app, &h.who, &eps[0].id).await.unwrap();
    let binding: String = sqlx::query_scalar(
        "SELECT play_id FROM fntv_record_links WHERE auth_hash=? AND link_hash=?",
    )
    .bind(&h.who.auth_hash)
    .bind(hash(sdk))
    .fetch_one(&h.app.db)
    .await
    .unwrap();
    assert_eq!(binding, play.id);
    assert_eq!(play.mode, "proxy");
    let (_, recorded) = request(&h, "POST", "/v/api/v1/play/record", json!({"media_guid":eps[0].id,"play_link":sdk,"ts":15,"bitrate":stream["data"]["video_stream"]["bps"]}), Some(&h.raw)).await;
    assert_eq!(recorded["data"], true);
    assert_eq!(
        playback::get(&h.app, &play.id).await.unwrap().position,
        15.0
    );
    let (_, files) = request(
        &h,
        "GET",
        &format!("/v/api/v1/stream/list/{}", eps[0].id),
        json!({}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(files["data"]["playback_mode"], "proxy");
    assert!(text(&files["data"]["files"][0], "file_name").starts_with("转发 · "));
    playback::set_forbidden(&h.app, true).await.unwrap();
    for method in ["GET", "HEAD"] {
        assert_eq!(
            request(&h, method, &path, json!({}), Some(&h.raw)).await.0,
            403
        );
    }
}

#[tokio::test]
async fn native_hls_new_play_does_not_reuse_a_pending_proxy_retry_after_forbidding_proxy() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let old = playback::latest(&h.app, &h.who, &eps[0].id).await.unwrap();
    playback::fallback(&h.app, &h.who, &old.id, "manual")
        .await
        .unwrap();
    playback::set_forbidden(&h.app, true).await.unwrap();
    let (status, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        stream["data"]["cloud_storage_info"]["cloud_storage_type"],
        9001
    );
    let current = playback::latest(&h.app, &h.who, &eps[0].id).await.unwrap();
    assert_ne!(current.id, old.id);
    assert_eq!(current.mode, "direct");
    assert!(playback::get(&h.app, &old.id).await.unwrap().ended);
}

#[tokio::test]
async fn native_menu_respects_https_reverse_proxy_and_media_extensions() {
    let h = harness().await;
    let source = catalog::sources(&h.app).await.unwrap().remove(0);
    let url = catalog::episode(&h.app, &h.episode).await.unwrap().url;
    let film = catalog::ingest(
        &h.app,
        &source,
        &json!({"vod_id":7101,"vod_name":"HTTPS交付验收","vod_year":"2026",
        "vod_play_from":"线路A$$$线路B","vod_play_url":format!("正片${url}$$$正片${url}")}),
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let eps = catalog::episodes(&h.app, &film.id).await.unwrap();
    for e in &eps {
        cached_native_media(&h, &e.id, 1280, 720).await;
    }
    let response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v/api/v1/stream")
                .header("host", "127.0.0.1:3000")
                .header("x-forwarded-host", "video.example:8443")
                .header("x-forwarded-proto", "https")
                .header("authorization", format!("Bearer {}", h.raw))
                .body(Body::from(json!({"media_guid":eps[0].id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let data: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    for choice in data["data"]["direct_link_qualities"].as_array().unwrap() {
        let link = url::Url::parse(&text(choice, "url")).unwrap();
        assert_eq!(
            link.origin().ascii_serialization(),
            "https://video.example:8443"
        );
        assert!(link.path().ends_with(".mp4"));
        assert!(text(choice, "resolution").starts_with("测试源/线路"));
        assert!(!text(choice, "resolution").contains('·'));
        assert!(!text(choice, "resolution").contains('\n'));
    }
    assert_eq!(
        data["data"]["cloud_storage_info"]["cloud_storage_type"],
        9001
    );
}

#[tokio::test]
async fn native_nas_retry_streams_media_and_honors_the_global_proxy_policy() {
    let h = harness().await;
    request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":h.episode}),
        Some(&h.raw),
    )
    .await;
    let path = format!("/v/api/v1/media/range/{}", h.episode);
    let response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("host", "127.0.0.1:3000")
                .header("authorization", format!("Bearer {}", h.raw))
                .header("cookie", "Trim-MC-token=must-not-reach-source")
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        206,
        "原生NAS代理播放应就地建立当前会话的转发"
    );
    assert_eq!(response.headers()["content-range"], "bytes 2-5/8");
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "2345"
    );
    let p = playback::latest(&h.app, &h.who, &h.episode).await.unwrap();
    assert_eq!(p.mode, "proxy");
    let menus_before: i64 = sqlx::query_scalar("SELECT count(*) FROM fntv_playback_menus")
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    let (_, recorded) = request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({
            "media_guid":h.episode,"play_link":"12345678-1234-1234-1234-123456789abc","ts":15
        }),
        Some(&h.raw),
    )
    .await;
    assert_eq!(recorded["data"], true);
    assert_eq!(
        request(&h, "GET", &path, json!({}), Some(&h.raw)).await.0,
        200
    );
    let menus_after: i64 = sqlx::query_scalar("SELECT count(*) FROM fntv_playback_menus")
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(menus_after, menus_before, "重复读取不能替换原生进度关联");
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &h.film, false)
            .await
            .unwrap()["progress"]["position"],
        15.0
    );
    playback::set_forbidden(&h.app, true).await.unwrap();
    assert_eq!(
        request(&h, "GET", &path, json!({}), Some(&h.raw)).await.0,
        403
    );
}

#[tokio::test]
async fn native_hls_rpc_reuses_its_manifest_link_and_proxies_all_child_resources() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, true).await;
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let (_, played) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let link = text(&played["data"], "play_link");
    let current_code = &stream["data"]["video_stream"]["bps"];
    let target_quality = stream["data"]["qualities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quality| &quality["bitrate"] != current_code)
        .unwrap();
    let (_, rpc) = request(
        &h,
        "POST",
        "/v/api/v1/media/p",
        json!({"req":"media.resetQuality","reqid":"native-hls-switch",
        "playLink":link,"quality":target_quality}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(rpc["data"]["result"], "succ");
    assert_eq!(
        rpc["data"]["play_link"], link,
        "HLS在线换源要保持原播放清单地址有效"
    );
    let url = url::Url::parse("http://127.0.0.1:3000")
        .unwrap()
        .join(&link)
        .unwrap();
    let response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(url.path())
                .header("authorization", format!("Bearer {}", h.raw))
                .header("range", "bytes=0-0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let manifest = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(manifest.starts_with("#EXTM3U"));
    assert!(
        !manifest.contains("#EXT-X-KEY:"),
        "原生转发交付原画解密分片"
    );
    assert!(!manifest.contains("secret=1"));
    assert!(!manifest.contains("segment.ts"));
    let segment = manifest
        .lines()
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap();
    assert!(
        segment.starts_with("http://127.0.0.1:3000/media/"),
        "原生分片地址须包含完整服务地址"
    );
    let segment_response = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .uri(segment)
                .header("range", "bytes=0-")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(segment_response.status(), 200);
    assert_eq!(segment_response.headers()["content-type"], "video/mp2t");
    assert!(!segment_response.headers().contains_key("content-range"));
    let data = segment_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(data.first(), Some(&0x47));
    assert_eq!(data.len() % 188, 0, "解密后保留完整的 MPEG-TS 包");
    let media = h._dir.path().join("native-decrypted.ts");
    tokio::fs::write(&media, data).await.unwrap();
    let probed = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(media)
        .output()
        .await
        .unwrap();
    assert!(probed.status.success());
    let streams: Value = serde_json::from_slice(&probed.stdout).unwrap();
    let video = streams["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["width"], 64);
    assert_eq!(video["height"], 64);
    let active = playback::latest(&h.app, &h.who, &eps[1].id).await.unwrap();
    assert_eq!(active.mode, "proxy");
    assert!(playback::latest(&h.app, &h.who, &eps[0].id).await.is_err());
    let (_, selected) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[1].id,"bitrate":target_quality["bitrate"]}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(
        selected["data"]["play_link"], link,
        "换源后当前媒体仍解析同一菜单"
    );
    playback::set_forbidden(&h.app, true).await.unwrap();
    let head = router(h.app.clone(), "/nonexistent")
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(url.path())
                .header("authorization", format!("Bearer {}", h.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), 403, "认证媒体HEAD同样服从禁止转发设置");
}

#[tokio::test]
async fn expired_media_is_reported_before_playback_and_failed_switch_keeps_the_old_source() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    let gone = url::Url::parse(&eps[1].url)
        .unwrap()
        .join("/gone.m3u8")
        .unwrap();
    sqlx::query("UPDATE episodes SET url=?,probe='',probed_at=0 WHERE id=?")
        .bind(gone.as_str())
        .bind(&eps[1].id)
        .execute(&h.app.db)
        .await
        .unwrap();
    assert!(
        playback::start(&h.app, &h.who, &eps[1].id)
            .await
            .unwrap_err()
            .1
            .contains("媒体地址已失效")
    );
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let (_, played) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let target = stream["data"]["qualities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quality| quality["bitrate"] != stream["data"]["video_stream"]["bps"])
        .unwrap();
    let (status, failed) = request(
        &h,
        "POST",
        "/v/api/v1/media/p",
        json!({"req":"media.resetQuality","reqid":"expired-switch",
        "playLink":played["data"]["play_link"],"quality":target}),
        Some(&h.raw),
    )
    .await;
    assert_eq!(status, 400);
    assert!(text(&failed, "msg").contains("媒体地址已失效"));
    assert!(
        !playback::latest(&h.app, &h.who, &eps[0].id)
            .await
            .unwrap()
            .ended
    );
    assert!(playback::latest(&h.app, &h.who, &eps[1].id).await.is_err());
    for path in [
        "/v/api/v1/play/info",
        "/v/api/v1/stream",
        "/v/api/v1/play/play",
    ] {
        let (status, response) = request(
            &h,
            "POST",
            path,
            json!({"media_guid":eps[1].id}),
            Some(&h.raw),
        )
        .await;
        assert_eq!(status, 400);
        assert!(text(&response, "msg").contains("媒体地址已失效"));
    }
}

#[tokio::test]
async fn native_hls_api_switch_does_not_let_the_old_client_quit_end_the_new_playback() {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let code = stream["data"]["video_stream"]["bps"].as_u64().unwrap();
    let target = stream["data"]["qualities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quality| quality["bitrate"] != code)
        .unwrap();
    let sdk_id = "12345678-1234-1234-1234-123456789abc";
    let old_report = |ts| json!({"media_guid":eps[0].id,"play_link":sdk_id,"ts":ts,"bitrate":code});
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        old_report(12),
        Some(&h.raw),
    )
    .await;
    let (_, switched) = request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[0].id,"bitrate":target["bitrate"],"startTimestamp":12}),
        Some(&h.raw),
    )
    .await;
    let link = text(&switched["data"], "play_link");
    let unbound_old = "12345678-1234-1234-1234-123456789abe";
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        json!({"media_guid":eps[0].id,"play_link":unbound_old,"ts":99,"bitrate":code}),
        Some(&h.raw),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/quit",
        json!({"media_guid":eps[0].id,"play_link":unbound_old}),
        Some(&h.raw),
    )
    .await;
    let old_quit = json!({"media_guid":eps[0].id,"play_link":sdk_id});
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/quit",
            old_quit.clone(),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    assert!(
        !playback::latest(&h.app, &h.who, &eps[1].id)
            .await
            .unwrap()
            .ended
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        old_report(99),
        Some(&h.raw),
    )
    .await;
    let path = url::Url::parse("http://127.0.0.1:3000")
        .unwrap()
        .join(&link)
        .unwrap()
        .path()
        .to_owned();
    assert_eq!(
        request(&h, "GET", &path, json!({}), Some(&h.raw)).await.0,
        200
    );
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/record",
            json!({"media_guid":eps[0].id,"play_link":link,"ts":20,"bitrate":target["bitrate"]}),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &eps[0].film_id, false)
            .await
            .unwrap()["progress"]["position"],
        20.0
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/play",
        json!({"media_guid":eps[1].id,"bitrate":code}),
        Some(&h.raw),
    )
    .await;
    request(&h, "POST", "/v/api/v1/play/quit", old_quit, Some(&h.raw)).await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        old_report(99),
        Some(&h.raw),
    )
    .await;
    let current = playback::latest(&h.app, &h.who, &eps[0].id).await.unwrap();
    assert!(
        !current.ended,
        "换回相同集号与线路后也不能接受旧SDK退出通知"
    );
    assert_eq!(current.reports, 0);
}

#[tokio::test]
async fn native_hls_client_record_ids_save_progress_and_remain_isolated_after_switch_delete_and_replay()
 {
    let h = harness().await;
    let eps = native_hls_fixture(&h, false).await;
    let (_, stream) = request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    let code = stream["data"]["video_stream"]["bps"].as_u64().unwrap();
    let other = stream["data"]["qualities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quality| quality["bitrate"] != code)
        .unwrap();
    let sdk_id = "12345678-1234-1234-1234-123456789abc";
    let (_, item) = request(
        &h,
        "GET",
        &format!("/v/api/v1/item/{}", eps[0].film_id),
        json!({}),
        Some(&h.raw),
    )
    .await;
    let canonical = text(&item["data"], "guid");
    let report = |ts, bitrate| json!({"item_guid":eps[0].id,"media_guid":eps[0].id,"play_link":sdk_id,"ts":ts,"bitrate":bitrate});
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/play/record",
            json!({"item_guid":canonical,"play_link":sdk_id,"ts":12,"bitrate":code}),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &eps[0].film_id, false)
            .await
            .unwrap()["progress"]["position"],
        12.0
    );
    let reset = |quality: Value| json!({"req":"media.resetQuality","reqid":"switch","playLink":sdk_id,"quality":quality});
    assert_eq!(
        request(
            &h,
            "POST",
            "/v/api/v1/media/p",
            reset(other.clone()),
            Some(&h.raw)
        )
        .await
        .0,
        200
    );
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        report(99, code),
        Some(&h.raw),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        report(20, other["bitrate"].as_u64().unwrap()),
        Some(&h.raw),
    )
    .await;
    let switched = catalog::view(&h.app, &h.who.user.id, &eps[0].film_id, false)
        .await
        .unwrap();
    assert_eq!(switched["progress"]["episode_id"], eps[1].id);
    assert_eq!(switched["progress"]["position"], 20.0);
    let original_quality = stream["data"]["qualities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quality| quality["bitrate"] == code)
        .unwrap();
    request(
        &h,
        "POST",
        "/v/api/v1/media/p",
        reset(original_quality.clone()),
        Some(&h.raw),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        report(27, code),
        Some(&h.raw),
    )
    .await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &eps[0].film_id, false)
            .await
            .unwrap()["progress"]["position"],
        27.0,
        "换回旧线路仍能记录，不能把清单地址当作重复播放身份"
    );
    assert!(
        catalog::personal(&h.app, &h.other.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    playback::remove_history(
        &h.app,
        &h.who.user.id,
        std::slice::from_ref(&eps[0].film_id),
    )
    .await
    .unwrap();
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        report(30, code),
        Some(&h.raw),
    )
    .await;
    assert!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .is_empty()
    );
    request(
        &h,
        "POST",
        "/v/api/v1/stream",
        json!({"media_guid":eps[0].id}),
        Some(&h.raw),
    )
    .await;
    request(
        &h,
        "POST",
        "/v/api/v1/play/record",
        report(80, code),
        Some(&h.raw),
    )
    .await;
    assert!(
        catalog::personal(&h.app, &h.who.user.id, "history")
            .await
            .unwrap()
            .is_empty(),
        "旧原生记录身份不能落入重新起播的菜单"
    );
    request(&h,"POST","/v/api/v1/play/record",json!({"media_guid":eps[0].id,"play_link":"12345678-1234-1234-1234-123456789abd","ts":10,"bitrate":code}),Some(&h.raw)).await;
    assert_eq!(
        catalog::view(&h.app, &h.who.user.id, &eps[0].film_id, false)
            .await
            .unwrap()["progress"]["position"],
        10.0
    );
}
