use crate::errors::ApiError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use callmind_core::CallId;
use callmind_search::AskCallsRequest;
use callmind_ui::{
    AwaitedPlugin, render_analytics_dashboard, render_ask_page, render_call_detail,
    render_calls_list,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CallsListQuery {
    pub q: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub status: Option<String>,
    pub language: Option<String>,
    pub date: Option<String>,
}

pub async fn root_redirect() -> Redirect {
    Redirect::temporary("/calls")
}

pub async fn calls_page(
    State(state): State<AppState>,
    Query(query): Query<CallsListQuery>,
) -> Result<Html<String>, ApiError> {
    let current_page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(10, 100);
    let offset = (current_page - 1) * page_size;

    let status_str = match query.status.as_deref() {
        Some("completed") => Some("completed"),
        Some("pending") => Some("pending"),
        Some("processing") => Some("processing"),
        Some("failed") => Some("failed"),
        _ => None,
    };

    let lang_filter = query.language.as_deref().filter(|l| *l != "all");
    let date_filter = query.date.as_deref().filter(|d| *d != "all");
    let search_q = query.q.as_deref().filter(|q| !q.trim().is_empty());
    let (rows, total_count) = state
        .call_repo
        .list_for_display(&callmind_db::CallListFilter {
            status: status_str,
            language: lang_filter,
            date: date_filter,
            search: search_q,
            limit: page_size,
            offset,
        })
        .await?;

    let items: Vec<callmind_ui::CallListItem> = rows
        .into_iter()
        .map(|row| {
            // The badge now comes from the same indexed column the filter uses.
            // It used to be re-derived by sniffing characters in the preview
            // text, so a call could be filtered as Hebrew and shown as English.
            let language = match row.primary_language.as_deref() {
                Some("hebrew" | "he") => Some("he".to_string()),
                Some("russian" | "ru") => Some("ru".to_string()),
                Some("english" | "en") => Some("en".to_string()),
                Some(other) if !other.is_empty() => Some(other.to_string()),
                _ => None,
            };
            callmind_ui::CallListItem {
                call: row.call,
                language,
            }
        })
        .collect();

    let total_pages = ((total_count as f64) / (page_size as f64)).ceil() as u32;

    let pagination_info = callmind_ui::PaginationInfo {
        current_page,
        page_size,
        total_count: total_count as u64,
        total_pages: total_pages.max(1),
        status_filter: query.status.clone(),
        language_filter: query.language.clone(),
        date_filter: query.date.clone(),
        query_search: query.q.clone(),
    };

    let html = render_calls_list(&items, &pagination_info);
    Ok(Html(html))
}

pub async fn call_detail_page(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
) -> Result<Response, ApiError> {
    let call = state
        .call_repo
        .get_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Call {id} not found")))?;

    let analysis: Option<callmind_analysis::CallAnalysis> = state
        .call_repo
        .get_analysis_json(id)
        .await?
        .and_then(|(_title, json)| serde_json::from_str(&json).ok());

    let transcript: Option<callmind_transcript::Transcript> = state
        .call_repo
        .get_transcript_json(id)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok());

    let last_error = state.stats_repo.last_job_error(id).await?;

    let plugin_results = state.call_repo.list_plugin_results(id).await?;

    // A plugin job for a kind no worker leases is never reaped -- the reaper
    // only reclaims leases, and nobody ever took this one -- so it sits pending
    // while the call completes and the page looks finished. Name it instead.
    let reported: std::collections::HashSet<&str> = plugin_results
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let awaited: Vec<AwaitedPlugin> = state
        .job_repo
        .list_by_call_id(id)
        .await?
        .into_iter()
        .filter_map(|job| match &job.kind {
            callmind_core::JobKind::Custom(name) if !reported.contains(name.as_str()) => {
                Some(AwaitedPlugin {
                    plugin: name.clone(),
                    status: job.status,
                    since: job.created_at,
                    error: job.last_error,
                })
            }
            _ => None,
        })
        .collect();

    let html = render_call_detail(
        &call,
        transcript.as_ref(),
        analysis.as_ref(),
        last_error.as_deref(),
        &plugin_results,
        &awaited,
        &state.templates,
    );
    Ok(Html(html).into_response())
}

pub async fn analytics_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let stats = state.stats_repo.call_stats().await?;
    let total_calls = stats.total as u64;
    let completed_calls = stats.completed as u64;
    let avg_duration_secs = stats.avg_duration_ms / 1000.0;
    let total_audio_hours = stats.total_duration_ms / 3_600_000.0;

    let top_rows = state.stats_repo.top_intents(5).await?;

    let total_analyzed = top_rows.iter().map(|(_, c)| *c).sum::<i64>().max(1) as f64;
    let top_intents = top_rows
        .into_iter()
        .map(|(intent, count)| {
            let pct = ((count as f64 / total_analyzed) * 100.0).round() as u32;
            (intent, count as u64, pct)
        })
        .collect();

    // Daily activity counts from real calls
    let daily_rows = state.stats_repo.daily_call_counts(7).await?;

    let mut daily_counts: Vec<(String, u64)> =
        daily_rows.into_iter().map(|(d, c)| (d, c as u64)).collect();
    daily_counts.reverse();

    // Real dynamic language distribution from transcripts
    let lang_rows = state.stats_repo.language_distribution().await?;

    let mut hebrew_count = 0i64;
    let mut russian_count = 0i64;
    let mut english_count = 0i64;

    // Transcripts store the full language name ("hebrew"), while a language
    // hint uses the code ("he"). Accept both: matching only the code is how the
    // old query silently reported every call as English.
    for (lang, count) in lang_rows {
        match lang.as_str() {
            "he" | "hebrew" => hebrew_count += count,
            "ru" | "russian" => russian_count += count,
            _ => english_count += count,
        }
    }

    let total_lang = (hebrew_count + russian_count + english_count).max(1) as f64;
    let hebrew_percent = ((hebrew_count as f64 / total_lang) * 100.0).round() as u32;
    let russian_percent = ((russian_count as f64 / total_lang) * 100.0).round() as u32;
    let english_percent = (100u32).saturating_sub(hebrew_percent + russian_percent);

    let analytics_data = callmind_ui::AnalyticsData {
        total_calls,
        completed_calls,
        avg_duration_secs,
        total_audio_hours,
        hebrew_percent,
        russian_percent,
        english_percent,
        top_intents,
        daily_counts,
    };

    let html = render_analytics_dashboard(&analytics_data);
    Ok(Html(html))
}

#[derive(Debug, Deserialize)]
pub struct AskQuery {
    pub q: Option<String>,
}

pub async fn ask_page(
    State(state): State<AppState>,
    Query(query): Query<AskQuery>,
) -> Result<Html<String>, ApiError> {
    let query_str = query.q.as_deref();
    let response = if let Some(q) = query_str {
        if q.trim().is_empty() {
            None
        } else {
            state
                .ask
                .ask(AskCallsRequest {
                    question: q.to_string(),
                    organization_id: None,
                    max_sources: Some(5),
                })
                .await
                .ok()
        }
    } else {
        None
    };

    let html = render_ask_page(query_str, response.as_ref());
    Ok(Html(html))
}
