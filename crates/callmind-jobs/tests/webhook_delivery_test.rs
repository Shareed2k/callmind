//! Outbound delivery of a finished call, driven through a real HTTP server
//! rather than a mocked client: what matters is the bytes a receiver such as
//! n8n or a Shortcut actually sees.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use callmind_core::{Job, JobId, JobKind, JobStatus};
use callmind_jobs::{JobContext, JobExecutionError, JobHandler, WebhookDeliveryHandler};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Received {
    calls: Arc<Mutex<Vec<(HeaderMap, serde_json::Value)>>>,
}

async fn record(
    State(state): State<Received>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, &'static str) {
    let value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    state.calls.lock().expect("lock").push((headers, value));
    (StatusCode::OK, "ok")
}

/// Serves `handler` on an ephemeral port and returns its URL.
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("addr");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}/hook"), task)
}

fn job(payload: serde_json::Value) -> JobContext {
    JobContext {
        job: Job {
            id: JobId::generate(),
            call_id: None,
            kind: JobKind::DeliverWebhook,
            payload,
            status: JobStatus::Running,
            priority: 0,
            attempt: 1,
            max_attempts: 5,
            run_after: chrono::Utc::now(),
            locked_at: None,
            locked_by: None,
            last_error: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        },
        cancellation_token: CancellationToken::new(),
    }
}

#[tokio::test]
async fn it_delivers_the_payload_verbatim_with_the_shared_secret() {
    let received = Received::default();
    let (url, task) = serve(
        Router::new()
            .route("/hook", post(record))
            .with_state(received.clone()),
    )
    .await;

    let payload = serde_json::json!({
        "event": "call.completed",
        "call_id": "8f14e45f-ceea-467a-9a36-dedd4bea2543",
        "title": "Тестовый разговор",
        "summary": "Договорились созвониться завтра.",
    });

    WebhookDeliveryHandler::new(url, Some("s3cret".to_string()), Duration::from_secs(5))
        .execute(job(payload.clone()))
        .await
        .expect("a 200 response is a delivered webhook");

    let calls = received.calls.lock().expect("lock");
    assert_eq!(calls.len(), 1, "exactly one delivery");
    let (headers, body) = &calls[0];
    assert_eq!(
        body, &payload,
        "the receiver sees the payload verbatim, so a retry is byte-identical"
    );
    assert_eq!(
        headers
            .get("x-callmind-secret")
            .map(|v| v.to_str().unwrap_or_default()),
        Some("s3cret"),
        "the receiver has to be able to tell it is us"
    );
    assert_eq!(
        headers
            .get("content-type")
            .map(|v| v.to_str().unwrap_or_default()),
        Some("application/json"),
    );
    task.abort();
}

#[tokio::test]
async fn a_server_error_is_retryable_so_the_queue_backs_off() {
    let (url, task) = serve(Router::new().route(
        "/hook",
        post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    ))
    .await;

    let err = WebhookDeliveryHandler::new(url, None, Duration::from_secs(5))
        .execute(job(serde_json::json!({"event": "call.completed"})))
        .await
        .expect_err("a 500 must not count as delivered");

    match err {
        JobExecutionError::Retryable(msg) => {
            assert!(msg.contains("500"), "the status is worth logging: {msg}");
        }
        other => panic!("a failing receiver deserves a retry, got {other:?}"),
    }
    task.abort();
}

/// A 4xx means the request itself is wrong -- a bad URL, a rejected secret --
/// and replaying it unchanged cannot fix that.
#[tokio::test]
async fn a_rejected_request_is_permanent_so_the_queue_stops_replaying_it() {
    let (url, task) = serve(Router::new().route(
        "/hook",
        post(|| async { (StatusCode::UNAUTHORIZED, "nope") }),
    ))
    .await;

    let err = WebhookDeliveryHandler::new(url, None, Duration::from_secs(5))
        .execute(job(serde_json::json!({"event": "call.completed"})))
        .await
        .expect_err("a 401 must not count as delivered");

    assert!(
        matches!(err, JobExecutionError::Failed(_)),
        "retrying a rejected request just repeats it, got {err:?}"
    );
    task.abort();
}
