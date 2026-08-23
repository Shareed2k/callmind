use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use callmind_db::DbError;
use callmind_storage::StorageError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Resource conflict: {0}")]
    Conflict(String),

    #[error("Payload too large: {0}")]
    PayloadTooLarge(String),

    #[error("Invalid range request: {0}")]
    RangeNotSatisfiable(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiErrorResponse {
    pub error: ErrorDetail,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "unauthorized", msg),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg),
            Self::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", msg),
            Self::RangeNotSatisfiable(msg) => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                "range_not_satisfiable",
                msg,
            ),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", msg),
        };

        let body = Json(ApiErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message,
            },
        });

        (status, body).into_response()
    }
}

impl From<callmind_search::SearchError> for ApiError {
    fn from(err: callmind_search::SearchError) -> Self {
        match err {
            callmind_search::SearchError::InvalidQuery(msg) => ApiError::BadRequest(msg),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound(msg) => ApiError::NotFound(msg),
            DbError::ConstraintViolation(msg) => ApiError::Conflict(msg),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound(msg) => ApiError::NotFound(msg),
            StorageError::InvalidKey(msg) => ApiError::BadRequest(msg),
            StorageError::QuotaExceeded(msg) => ApiError::PayloadTooLarge(msg),
            StorageError::Io { source, .. } => ApiError::Internal(source.to_string()),
        }
    }
}
