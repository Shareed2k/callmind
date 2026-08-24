use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use callmind_analysis::AnalysisEngine;
use callmind_api::{AppState, create_router};
use callmind_config::AppConfig;
use callmind_core::{Call, CreateCallRequest};
use callmind_db::{
    SqlCallRepository, SqlJobRepository, SqlSearchIndex, SqlStatsRepository, create_sqlite_pool,
    orm_connection, run_migrations,
};
use callmind_llm::MockLlmEngine;
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::FilesystemStorage;
use http_body_util::BodyExt;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

async fn setup_test_app() -> (axum::Router, tempfile::TempDir) {
    setup_test_app_with_config(AppConfig::default()).await
}

async fn setup_test_app_with_config(config: AppConfig) -> (axum::Router, tempfile::TempDir) {
    let pool = create_sqlite_pool(":memory:", 5).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let temp_dir = tempdir().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path()).await.unwrap();

    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));
    let job_repo = Arc::new(SqlJobRepository::new(orm_connection(&pool)));
    let stats_repo = Arc::new(SqlStatsRepository::new(orm_connection(&pool)));
    let storage_arc = Arc::new(storage);
    let config = Arc::new(config);

    let search_engine = Arc::new(SearchEngine::new(Arc::new(SqlSearchIndex::new(
        orm_connection(&pool),
    ))));
    let mock_llm = Arc::new(MockLlmEngine::default());
    let ask_engine = Arc::new(AskEngine::new((*search_engine).clone(), mock_llm.clone()));
    let analysis_engine = Arc::new(AnalysisEngine::new(mock_llm));

    let state = AppState::new(
        config,
        call_repo.clone(),
        call_repo,
        job_repo,
        stats_repo,
        storage_arc,
        search_engine,
        ask_engine,
        analysis_engine,
        Arc::new(callmind_ui::templates::TemplateRegistry::new()),
    );
    (create_router(state), temp_dir)
}

#[tokio::test]
async fn test_health_and_ready_endpoints() {
    let (app, _dir) = setup_test_app().await;

    // Test /health
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Test /ready
    let req = Request::builder()
        .uri("/ready")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_call_lifecycle_and_recording_upload_stream() {
    let (app, _dir) = setup_test_app().await;

    // 1. Create Call
    let create_req = CreateCallRequest {
        external_id: Some("pbx-1001".to_string()),
        direction: None,
        phone_from: Some("+972501112233".to_string()),
        phone_to: Some("+97235559988".to_string()),
        started_at: None,
        channel_mapping: None,
    };
    let req_body = serde_json::to_vec(&create_req).unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/calls")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(req_body))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let created_call: Call = serde_json::from_slice(&body_bytes).unwrap();
    let call_id = created_call.id;

    // 2. Upload Audio Recording
    let valid_audio_buffer = callmind_audio::AudioBuffer::new(16000, 1, vec![0.1; 16000]);
    let fake_audio_data = valid_audio_buffer.to_wav_bytes();
    let upload_req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/calls/{call_id}/recording"))
        .header(header::CONTENT_TYPE, "audio/wav")
        .body(Body::from(fake_audio_data.clone()))
        .unwrap();

    let upload_res = app.clone().oneshot(upload_req).await.unwrap();
    assert_eq!(upload_res.status(), StatusCode::ACCEPTED);

    // 3. Download Full Audio Recording
    let download_req = Request::builder()
        .uri(format!("/api/v1/calls/{call_id}/recording"))
        .body(Body::empty())
        .unwrap();

    let download_res = app.clone().oneshot(download_req).await.unwrap();
    assert_eq!(download_res.status(), StatusCode::OK);
    let downloaded_bytes = download_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(downloaded_bytes.as_ref(), fake_audio_data.as_slice());

    // 4. Download with HTTP Range Header (Bytes 0-9)
    let range_req = Request::builder()
        .uri(format!("/api/v1/calls/{call_id}/recording"))
        .header(header::RANGE, "bytes=0-9")
        .body(Body::empty())
        .unwrap();

    let range_res = app.clone().oneshot(range_req).await.unwrap();
    assert_eq!(range_res.status(), StatusCode::PARTIAL_CONTENT);
    let range_bytes = range_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(range_bytes.as_ref(), &fake_audio_data[0..10]);

    // 5. Delete Call
    let del_req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/calls/{call_id}"))
        .body(Body::empty())
        .unwrap();

    let del_res = app.clone().oneshot(del_req).await.unwrap();
    assert_eq!(del_res.status(), StatusCode::NO_CONTENT);

    // 6. Verify Call is Gone
    let get_req = Request::builder()
        .uri(format!("/api/v1/calls/{call_id}"))
        .body(Body::empty())
        .unwrap();

    let get_res = app.oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_ui_views_rendering() {
    let (app, _dir) = setup_test_app().await;

    // 1. Root redirect to /calls
    let req_root = Request::builder().uri("/").body(Body::empty()).unwrap();
    let res_root = app.clone().oneshot(req_root).await.unwrap();
    assert_eq!(res_root.status(), StatusCode::TEMPORARY_REDIRECT);

    // 2. /calls page
    let req_calls = Request::builder()
        .uri("/calls")
        .body(Body::empty())
        .unwrap();
    let res_calls = app.clone().oneshot(req_calls).await.unwrap();
    assert_eq!(res_calls.status(), StatusCode::OK);
    let html = res_calls.into_body().collect().await.unwrap().to_bytes();
    let html_str = String::from_utf8_lossy(&html);
    assert!(html_str.contains("CallMind"));
    assert!(html_str.contains("Recorded Conversations"));

    // 3. /analytics page
    let req_analytics = Request::builder()
        .uri("/analytics")
        .body(Body::empty())
        .unwrap();
    let res_analytics = app.clone().oneshot(req_analytics).await.unwrap();
    assert_eq!(res_analytics.status(), StatusCode::OK);
    let analytics_html = res_analytics
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let analytics_str = String::from_utf8_lossy(&analytics_html);
    assert!(analytics_str.contains("Conversation Analytics"));
    assert!(analytics_str.contains("Hebrew (עברית)"));

    // 4. /ask page
    let req_ask = Request::builder().uri("/ask").body(Body::empty()).unwrap();
    let res_ask = app.oneshot(req_ask).await.unwrap();
    assert_eq!(res_ask.status(), StatusCode::OK);
}

#[test]
fn test_bot_response_formatter_and_ics() {
    let raw_json = serde_json::json!({
        "title": "Встреча у стоматолога и покупка продуктов",
        "summary": "Анна договорилась о визите к стоматологу на вторник и попросила купить молоко и хлеб.",
        "reason": "Запись на прием к врачу",
        "action_items": [
            { "text": "Купить молоко и свежий хлеб", "owner": "speaker_1", "deadline": "сегодня 18:00" },
            { "text": "Прийти на прием к стоматологу", "owner": "speaker_2", "deadline": "вторник 14:00" }
        ],
        "key_facts": [
            "Стоматология находится на 2 этаже",
            "Код домофона 4521"
        ],
        "entities": [
            { "entity_type": "location", "value": "ул. Ленина 45, каб. 12" },
            { "entity_type": "phone", "value": "+972501234567" }
        ]
    }).to_string();

    let call_id = "test-call-1234";
    let formatted =
        callmind_api::BotResponseFormatter::format(call_id, &raw_json, "127.0.0.1:8080");

    assert_eq!(formatted.title, "Встреча у стоматолога и покупка продуктов");
    assert!(formatted.text_markdown.contains("• [ ] Купить молоко"));
    assert!(
        formatted
            .text_markdown
            .contains("📍 *Key Facts & Details:*")
    );
    assert!(
        formatted
            .text_markdown
            .contains("http://127.0.0.1:8080/calls/test-call-1234")
    );

    assert!(formatted.has_calendar_event);
    assert!(formatted.ics_content.is_some());
    let ics = formatted.ics_content.unwrap();
    assert!(ics.contains("BEGIN:VCALENDAR"));
    assert!(ics.contains("LOCATION:ул. Ленина 45, каб. 12"));
    assert!(ics.contains("END:VCALENDAR"));
}

/// The UI routes used to sit outside the auth layer, so `auth.enabled = true`
/// locked the API while leaving every transcript readable at `/calls`.
#[tokio::test]
async fn test_auth_covers_ui_and_api_routes() {
    const KEY: &str = "test-secret-key";

    let mut config = AppConfig::default();
    config.auth.enabled = true;
    config.auth.api_key = Some(KEY.to_string());
    let (app, _dir) = setup_test_app_with_config(config).await;

    let get = |uri: &str, auth: Option<(&str, &str)>| {
        let mut builder = Request::builder().uri(uri.to_string());
        if let Some((name, value)) = auth {
            builder = builder.header(name, value.to_string());
        }
        let app = app.clone();
        let req = builder.body(Body::empty()).unwrap();
        async move { app.oneshot(req).await.unwrap() }
    };

    // Probes stay reachable without credentials.
    assert_eq!(get("/health", None).await.status(), StatusCode::OK);
    assert_eq!(get("/ready", None).await.status(), StatusCode::OK);

    // Every data-bearing route rejects an anonymous request.
    for uri in [
        "/calls",
        "/analytics",
        "/ask",
        "/api/v1/calls",
        "/api-docs/openapi.json",
    ] {
        let res = get(uri, None).await;
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} was not protected"
        );
        assert!(
            res.headers().contains_key(header::WWW_AUTHENTICATE),
            "{uri} must challenge the browser so the UI stays usable"
        );
    }

    // A wrong key is still rejected.
    assert_eq!(
        get("/calls", Some(("X-API-Key", "wrong-key")))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // All three credential forms are accepted. Basic is what lets a browser
    // reach the HTML UI, which cannot set custom headers by navigation.
    let basic = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("callmind:{KEY}")
        )
    );
    for auth in [
        ("X-API-Key", KEY.to_string()),
        (header::AUTHORIZATION.as_str(), format!("Bearer {KEY}")),
        (header::AUTHORIZATION.as_str(), basic),
    ] {
        let res = get("/calls", Some((auth.0, &auth.1))).await;
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "credential {} rejected",
            auth.1
        );
    }
}

/// The organization a call belongs to must not be selectable by the caller.
///
/// Authentication is a single shared API key -- there is no per-tenant principal
/// to check an organization against -- so honouring `organization_id` from the
/// body would let any authenticated caller place a call into, or read it out of,
/// somebody else's organization the moment a second one exists. That is an IDOR
/// primitive waiting for a tenant.
#[tokio::test]
async fn a_caller_cannot_choose_the_organization_a_call_belongs_to() {
    let (app, _tmp) = setup_test_app().await;

    let foreign = uuid::Uuid::new_v4();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/calls")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "organization_id": foreign.to_string(),
                        "direction": "incoming",
                        "external_id": "org-scoping-probe"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("request completes");

    assert!(
        response.status().is_success(),
        "the call is still created: {}",
        response.status()
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_ne!(
        created["organization_id"].as_str(),
        Some(foreign.to_string().as_str()),
        "the requested organization must be ignored, not honoured"
    );
    assert_eq!(
        created["organization_id"].as_str(),
        Some(callmind_core::OrgId::DEFAULT.to_string().as_str()),
        "the call belongs to the server's own organization"
    );
}
