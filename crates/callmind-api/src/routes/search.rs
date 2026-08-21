use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use callmind_search::{AskCallsRequest, AskCallsResponse, SearchFilter, SearchResultItem};

#[utoipa::path(
    get,
    path = "/api/v1/search",
    tag = "Search",
    params(SearchFilter),
    responses(
        (status = 200, description = "Search results from indexed calls", body = Vec<SearchResultItem>),
        (status = 400, description = "Invalid search query", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn search_calls(
    State(state): State<AppState>,
    Query(filter): Query<SearchFilter>,
) -> Result<Json<Vec<SearchResultItem>>, ApiError> {
    let results = state
        .search
        .search(&filter)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(results))
}

#[utoipa::path(
    post,
    path = "/api/v1/ask",
    tag = "Search",
    request_body = AskCallsRequest,
    responses(
        (status = 200, description = "AI synthesized answer citing call evidence", body = AskCallsResponse),
        (status = 400, description = "Invalid question request", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn ask_calls(
    State(state): State<AppState>,
    Json(payload): Json<AskCallsRequest>,
) -> Result<Json<AskCallsResponse>, ApiError> {
    let response = state
        .ask
        .ask(payload)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(response))
}
