use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct WhatsAppVerifyQuery {
    #[serde(rename = "hub.mode")]
    pub mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    pub verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    pub challenge: Option<String>,
}

/// Verification handshake endpoint for Meta WhatsApp Cloud API.
pub async fn verify_whatsapp_webhook(
    State(state): State<AppState>,
    Query(query): Query<WhatsAppVerifyQuery>,
) -> Result<Response, ApiError> {
    let Some(expected_token) = &state.config.bots.whatsapp.verify_token else {
        return Err(ApiError::Unauthorized(
            "WhatsApp verify token is not configured".into(),
        ));
    };

    if query.mode.as_deref() == Some("subscribe")
        && query.verify_token.as_deref() == Some(expected_token.as_str())
    {
        let challenge = query.challenge.unwrap_or_default();
        Ok((StatusCode::OK, challenge).into_response())
    } else {
        Err(ApiError::Unauthorized(
            "Invalid verification token or mode".into(),
        ))
    }
}

/// Incoming message webhook for WhatsApp Cloud API.
pub async fn handle_whatsapp_webhook(
    State(_state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Response, ApiError> {
    // Acknowledge receipt immediately as required by WhatsApp Cloud API (<3s)
    tracing::debug!("Received WhatsApp webhook payload: {:?}", payload);
    Ok((StatusCode::OK, "EVENT_RECEIVED").into_response())
}
