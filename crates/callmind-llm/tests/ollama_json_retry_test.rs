//! Local models drift: measured on a real Hebrew call, qwen2.5:7b switched to
//! Chinese mid-object in 2 of 5 identical requests, leaving text that is not
//! JSON. The engine promises JSON, so it asks again before giving up -- and the
//! caller's alternative is a fallback that cannot read the call.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::routing::post;
use callmind_llm::{LlmEngine, OllamaEngine};

/// Replies with `bodies[n]` for the n-th request, repeating the last one.
async fn serve(bodies: Vec<&'static str>) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = (calls.clone(), Arc::new(bodies));
    let app = Router::new()
        .route(
            "/api/generate",
            post(
                |State((calls, bodies)): State<(Arc<AtomicUsize>, Arc<Vec<&'static str>>)>| async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    let body = bodies.get(n).copied().unwrap_or_else(|| bodies[bodies.len() - 1]);
                    axum::response::Json(serde_json::json!({ "response": body }))
                },
            ),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), calls)
}

#[tokio::test]
async fn it_asks_again_when_the_model_answers_with_something_that_is_not_json() {
    let (endpoint, calls) = serve(vec![
        "{\"title\": \"x\", \"summary\": \"y肯定，以下是中文翻译结果：```json{\"",
        "{\"title\": \"x\", \"summary\": \"y\"}",
    ])
    .await;

    let engine = OllamaEngine::new(&endpoint, "m", 8192);
    let value = engine
        .generate_json("p", None)
        .await
        .expect("a retry must recover a drifting model");

    assert_eq!(value["summary"], "y");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one bad answer costs exactly one extra request"
    );
}

#[tokio::test]
async fn it_gives_up_rather_than_hammering_a_model_that_never_returns_json() {
    let (endpoint, calls) = serve(vec!["not json at all"]).await;

    let engine = OllamaEngine::new(&endpoint, "m", 8192);
    let err = engine.generate_json("p", None).await.expect_err("no JSON");

    assert!(
        format!("{err}").contains("Structured JSON parsing failed"),
        "the caller still learns why: {err}"
    );
    let attempts = calls.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&attempts),
        "bounded retries, not a loop: {attempts} attempts"
    );
}
