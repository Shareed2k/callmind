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
use axum::response::Response;
use axum::routing::{get, post};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

async fn auth_middleware(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !state.config.auth.enabled {
        return Ok(next.run(request).await);
    }

    let expected_key = match state.config.auth.api_key.as_deref() {
        Some(k) if !k.trim().is_empty() => k.trim(),
        _ => {
            return Err(ApiError::Unauthorized(
                "Authentication is enabled on server, but no valid API key is configured"
                    .to_string(),
            ));
        }
    };

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let x_api_key = headers.get("X-API-Key").and_then(|v| v.to_str().ok());

    let token_valid = if let Some(header) = auth_header {
        if let Some(token) = header.strip_prefix("Bearer ") {
            token.trim() == expected_key
        } else {
            header.trim() == expected_key
        }
    } else if let Some(key) = x_api_key {
        key.trim() == expected_key
    } else {
        false
    };

    if token_valid {
        Ok(next.run(request).await)
    } else {
        Err(ApiError::Unauthorized(
            "Missing or invalid API key / Bearer token".to_string(),
        ))
    }
}

/// Assemble the complete Axum API router.
pub fn create_router(state: AppState) -> Router {
    let body_limit_bytes = state.config.server.body_limit_mb * 1024 * 1024;

    let cors = if state.config.auth.allowed_origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let mut origins = Vec::new();
        for o in &state.config.auth.allowed_origins {
            if let Ok(header_val) = o.parse::<axum::http::HeaderValue>() {
                origins.push(header_val);
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
        .route(
            "/bots/whatsapp",
            get(crate::bots::verify_whatsapp_webhook).post(crate::bots::handle_whatsapp_webhook),
        )
        // System metrics route
        .route("/system/metrics", get(health::system_metrics))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        // Web UI Routes
        .route("/", get(ui::root_redirect))
        .route("/calls", get(ui::calls_page))
        .route("/calls/{id}", get(ui::call_detail_page))
        .route("/analytics", get(ui::analytics_page))
        .route("/ask", get(ui::ask_page))
        // OpenAPI specification and Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        // Health and Readiness
        .route("/health", get(health::health_check))
        .route("/ready", get(health::readiness_check))
        // API v1 prefix
        .nest("/api/v1", api_v1)
        .layer(DefaultBodyLimit::max(body_limit_bytes))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
