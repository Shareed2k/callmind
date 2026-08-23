pub mod calls;
pub mod health;
pub mod recordings;
pub mod search;
pub mod ui;

use crate::errors::ApiError;
use crate::openapi::ApiDoc;
use crate::state::AppState;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use subtle::ConstantTimeEq as _;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Constant-time secret comparison. Plain `==` on `&str` short-circuits on the
/// first differing byte, which leaks the secret to a timing attack.
fn secret_eq(candidate: &str, expected: &str) -> bool {
    candidate.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Extract the API key from an `Authorization: Basic` header, ignoring the
/// username. Browsers need this form: a plain navigation to `/calls` cannot set
/// `X-API-Key`, so without Basic the HTML UI would be unreachable whenever
/// authentication is enabled.
fn basic_auth_key(header: &str) -> Option<String> {
    let encoded = header.strip_prefix("Basic ")?;
    let decoded = BASE64_STANDARD.decode(encoded.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (_user, password) = text.split_once(':')?;
    Some(password.to_string())
}

/// 401 carrying `WWW-Authenticate`, so a browser prompts for credentials
/// instead of just showing a JSON error.
fn unauthorized(message: &str) -> Response {
    let mut response = ApiError::Unauthorized(message.to_string()).into_response();
    response.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static(r#"Basic realm="CallMind", charset="UTF-8""#),
    );
    response
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    if !state.config.auth.enabled {
        return next.run(request).await;
    }

    let expected_key = match state.config.auth.api_key.as_deref() {
        Some(k) if !k.trim().is_empty() => k.trim(),
        _ => {
            return unauthorized(
                "Authentication is enabled on server, but no valid API key is configured",
            );
        }
    };

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token_valid = if let Some(header) = auth_header {
        if let Some(token) = header.strip_prefix("Bearer ") {
            secret_eq(token.trim(), expected_key)
        } else if let Some(key) = basic_auth_key(header) {
            secret_eq(&key, expected_key)
        } else {
            secret_eq(header.trim(), expected_key)
        }
    } else if let Some(key) = headers.get("X-API-Key").and_then(|v| v.to_str().ok()) {
        secret_eq(key.trim(), expected_key)
    } else {
        false
    };

    if token_valid {
        next.run(request).await
    } else {
        unauthorized("Missing or invalid API key / Bearer token")
    }
}

/// Assemble the complete Axum API router.
pub fn create_router(state: AppState) -> Router {
    let body_limit_bytes = state
        .config
        .server
        .body_limit_mb
        .saturating_mul(1024 * 1024);

    let cors = if state.config.auth.allowed_origins.is_empty() {
        // Deny-by-default. `permissive()` here let any website read every
        // transcript; the server-rendered UI is same-origin and unaffected.
        CorsLayer::new()
    } else {
        let mut origins = Vec::new();
        for o in &state.config.auth.allowed_origins {
            match o.parse::<axum::http::HeaderValue>() {
                Ok(header_val) => origins.push(header_val),
                Err(e) => tracing::warn!("Ignoring unparseable CORS origin {o:?}: {e}"),
            }
        }
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    };

    let api_v1 = Router::new()
        // Calls routes
        .route("/calls", post(calls::create_call).get(calls::list_calls))
        .route("/calls/reanalyze-all", post(calls::reanalyze_all_calls))
        .route(
            "/calls/{id}",
            get(calls::get_call)
                .delete(calls::delete_call)
                .patch(calls::update_call),
        )
        .route("/calls/{id}/reprocess", post(calls::reprocess_call))
        .route("/calls/{id}/export", get(calls::export_transcript))
        .route("/calls/{id}/favorite", post(calls::toggle_call_favorite))
        .route(
            "/calls/{id}/tags",
            axum::routing::put(calls::update_call_tags),
        )
        // Recordings routes
        .route(
            "/calls/{id}/recording",
            post(recordings::upload_recording).get(recordings::get_recording),
        )
        // Search & Ask routes
        .route("/search", get(search::search_calls))
        .route("/ask", post(search::ask_calls))
        // Omnichannel Bot & Webhook routes (Siri, iOS Shortcuts, WhatsApp, n8n)
        .route("/bots/webhook", post(crate::bots::handle_audio_webhook))
        // Both shapes: Evolution appends the event name to the path when the
        // instance is configured with `webhookByEvents: true`.
        .route(
            "/bots/evolution",
            post(crate::bots::handle_evolution_webhook),
        )
        .route(
            "/bots/evolution/{event}",
            post(crate::bots::handle_evolution_webhook_by_event),
        )
        // System metrics route
        .route("/system/metrics", get(health::system_metrics));

    // Everything that can expose call data goes behind one auth layer. These
    // UI routes render full transcripts, and `/ask` spends the configured LLM
    // key, so leaving them outside the layer defeated `auth.enabled` entirely.
    let protected = Router::new()
        // Web UI Routes
        .route("/", get(ui::root_redirect))
        .route("/calls", get(ui::calls_page))
        .route("/calls/{id}", get(ui::call_detail_page))
        .route("/analytics", get(ui::analytics_page))
        .route("/ask", get(ui::ask_page))
        // OpenAPI specification and Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        // API v1 prefix
        .nest("/api/v1", api_v1)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        // Health and Readiness stay open for container probes
        .route("/health", get(health::health_check))
        .route("/ready", get(health::readiness_check))
        .merge(protected)
        .layer(DefaultBodyLimit::max(body_limit_bytes))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
