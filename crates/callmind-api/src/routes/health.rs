use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SystemMetricsResponse {
    /// Worker count from configuration, not a liveness probe: a worker task
    /// that died is still counted here.
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
    // One grouped scan per table instead of six sequential `count(*)` round
    // trips.
    let job_counts = state
        .stats_repo
        .job_counts_by_status()
        .await
        .map_err(|e| ApiError::Internal(format!("Database error reading job counts: {e}")))?;

    let mut pending_jobs = 0;
    let mut running_jobs = 0;
    let mut completed_jobs = 0;
    let mut failed_jobs = 0;
    for (status, count) in job_counts {
        match status.as_str() {
            "pending" => pending_jobs = count,
            "running" => running_jobs = count,
            "completed" => completed_jobs = count,
            "failed" => failed_jobs = count,
            _ => {}
        }
    }

    let stats = state
        .stats_repo
        .call_stats()
        .await
        .map_err(|e| ApiError::Internal(format!("Database error reading call counts: {e}")))?;
    let (total_calls, completed_calls) = (stats.total, stats.completed);

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

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct ReadyQuery {
    /// Also verify storage is writable by round-tripping a probe object.
    /// Off by default: a Kubernetes probe every 10s would otherwise create and
    /// delete a file on every single check.
    #[serde(default)]
    pub deep: bool,
}

#[utoipa::path(
    get,
    path = "/ready",
    tag = "System",
    params(ReadyQuery),
    responses(
        (status = 200, description = "System is ready for traffic", body = ReadyResponse),
        (status = 500, description = "System is not ready", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn readiness_check(
    State(state): State<AppState>,
    Query(query): Query<ReadyQuery>,
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

    let storage = if query.deep {
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

        // Was `let _ = delete`, which leaked a probe object on every failure.
        if let Err(e) = state.storage.delete(probe_key).await {
            warn!("Health probe object {probe_key} could not be removed: {e}");
        }
        "writable"
    } else {
        // Shallow check: the backend is configured and reachable enough to
        // answer an existence query.
        state
            .storage
            .exists(".health_probe")
            .await
            .map_err(|e| ApiError::Internal(format!("Storage health check failed: {e}")))?;
        "reachable"
    };

    Ok(Json(ReadyResponse {
        status: "ready".to_string(),
        database: "connected".to_string(),
        storage: storage.to_string(),
        workers: state.config.jobs.workers,
    }))
}
