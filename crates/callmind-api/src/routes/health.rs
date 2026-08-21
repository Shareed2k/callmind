use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SystemMetricsResponse {
    pub active_workers: usize,
    pub pending_jobs: i64,
    pub running_jobs: i64,
    pub completed_jobs: i64,
    pub failed_jobs: i64,
    pub total_calls: i64,
    pub completed_calls: i64,
}

#[utoipa::path(
    get,
    path = "/api/v1/system/metrics",
    tag = "System",
    responses(
        (status = 200, description = "Real-time background worker and queue metrics", body = SystemMetricsResponse)
    )
)]
pub async fn system_metrics(
    State(state): State<AppState>,
) -> Result<Json<SystemMetricsResponse>, ApiError> {
    let pending_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status = 'pending'")
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error in pending_jobs: {e}")))?;

    let running_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status = 'running'")
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error in running_jobs: {e}")))?;

    let completed_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status = 'completed'")
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error in completed_jobs: {e}")))?;

    let failed_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status = 'failed'")
        .fetch_one(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error in failed_jobs: {e}")))?;

    let total_calls: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error in total_calls: {e}")))?;

    let completed_calls: i64 =
        sqlx::query_scalar("SELECT count(*) FROM calls WHERE processing_status = 'completed'")
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error in completed_calls: {e}")))?;

    Ok(Json(SystemMetricsResponse {
        active_workers: state.config.jobs.workers,
        pending_jobs,
        running_jobs,
        completed_jobs,
        failed_jobs,
        total_calls,
        completed_calls,
    }))
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "System",
    responses(
        (status = 200, description = "System is alive", body = HealthResponse)
    )
)]
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReadyResponse {
    pub status: String,
    pub database: String,
    pub storage: String,
    pub workers: usize,
}

#[utoipa::path(
    get,
    path = "/ready",
    tag = "System",
    responses(
        (status = 200, description = "System is ready for traffic", body = ReadyResponse),
        (status = 500, description = "System is not ready", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn readiness_check(
    State(state): State<AppState>,
) -> Result<Json<ReadyResponse>, ApiError> {
    // Check DB
    let _ = state
        .call_repo
        .list(&callmind_core::CallFilter {
            limit: Some(1),
            ..Default::default()
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Database health check failed: {e}")))?;

    // Check Storage Read/Write capability
    let probe_key = ".health_probe";
    let probe_data = bytes::Bytes::from_static(b"health_check");
    let probe_stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(probe_data)
    }));

    state
        .storage
        .put(probe_key, probe_stream)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage write health check failed: {e}")))?;

    let _ = state.storage.delete(probe_key).await;

    Ok(Json(ReadyResponse {
        status: "ready".to_string(),
        database: "connected".to_string(),
        storage: "connected".to_string(),
        workers: state.config.jobs.workers,
    }))
}
