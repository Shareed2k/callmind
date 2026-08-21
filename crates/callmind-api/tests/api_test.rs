use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use callmind_analysis::AnalysisEngine;
use callmind_api::{AppState, create_router};
use callmind_config::AppConfig;
use callmind_core::{Call, CreateCallRequest};
use callmind_db::{SqliteCallRepository, SqliteJobRepository, create_sqlite_pool, run_migrations};
use callmind_llm::MockLlmEngine;
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::FilesystemStorage;
use http_body_util::BodyExt;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

async fn setup_test_app() -> (axum::Router, tempfile::TempDir) {
    let pool = create_sqlite_pool(":memory:", 5).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let temp_dir = tempdir().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path()).await.unwrap();

    let call_repo = Arc::new(SqliteCallRepository::new(pool.clone()));
    let job_repo = Arc::new(SqliteJobRepository::new(pool.clone()));
    let storage_arc = Arc::new(storage);
    let config = Arc::new(AppConfig::default());

    let search_engine = Arc::new(SearchEngine::new(pool.clone()));
    let mock_llm = Arc::new(MockLlmEngine::default());
    let ask_engine = Arc::new(AskEngine::new((*search_engine).clone(), mock_llm.clone()));
    let analysis_engine = Arc::new(AnalysisEngine::new(mock_llm));

    let state = AppState::new(
        config,
        call_repo,
        job_repo,
        storage_arc,
        search_engine,
        ask_engine,
        analysis_engine,
        pool,
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
        organization_id: None,
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
