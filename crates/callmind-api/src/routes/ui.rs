use crate::errors::ApiError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use callmind_core::{Call, CallDirection, CallId, OrgId, ProcessingStatus};
use callmind_search::AskCallsRequest;
use callmind_ui::{
    render_analytics_dashboard, render_ask_page, render_call_detail, render_calls_list,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::Row;
use std::str::FromStr;

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
    let search_like = search_q.map(|q| format!("%{q}%"));

    let count_query = r#"
        SELECT count(*)
        FROM calls c
        LEFT JOIN call_transcripts t ON c.id = t.call_id
        LEFT JOIN call_analyses a ON c.id = a.call_id
        WHERE (? IS NULL OR c.processing_status = ?)
          AND (
               ? IS NULL 
               OR (? = 'he' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('hebrew', 'he') OR t.transcript_json GLOB '*[א-ת]*'))
               OR (? = 'ru' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('russian', 'ru') OR t.transcript_json GLOB '*[а-яА-ЯёЁ]*'))
               OR (? = 'en' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('english', 'en') OR (t.transcript_json NOT GLOB '*[а-яА-ЯёЁ]*' AND t.transcript_json NOT GLOB '*[א-ת]*' AND length(t.transcript_json) > 100)))
          )
          AND (
               ? IS NULL
               OR (? = 'today' AND c.created_at >= date('now'))
               OR (? = '7d' AND c.created_at >= date('now', '-7 days'))
               OR (? = '30d' AND c.created_at >= date('now', '-30 days'))
          )
          AND (
               ? IS NULL
               OR c.external_id LIKE ?
               OR a.title LIKE ?
               OR t.transcript_json LIKE ?
          )
    "#;

    let total_count: i64 = sqlx::query_scalar(count_query)
        .bind(status_str)
        .bind(status_str)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let list_query = r#"
        SELECT c.id, c.organization_id, c.external_id, c.direction, c.phone_from, c.phone_to,
               c.started_at, c.ended_at, c.duration_ms, c.processing_status, c.is_favorite, c.tags,
               c.created_at, c.updated_at,
               json_extract(t.transcript_json, '$.segments[0].normalized_text') as sample_text,
               json_extract(t.transcript_json, '$.segments[0].language') as lang
        FROM calls c
        LEFT JOIN call_transcripts t ON c.id = t.call_id
        LEFT JOIN call_analyses a ON c.id = a.call_id
        WHERE (? IS NULL OR c.processing_status = ?)
          AND (
               ? IS NULL 
               OR (? = 'he' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('hebrew', 'he') OR t.transcript_json GLOB '*[א-ת]*'))
               OR (? = 'ru' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('russian', 'ru') OR t.transcript_json GLOB '*[а-яА-ЯёЁ]*'))
               OR (? = 'en' AND (json_extract(t.transcript_json, '$.segments[0].language') IN ('english', 'en') OR (t.transcript_json NOT GLOB '*[а-яА-ЯёЁ]*' AND t.transcript_json NOT GLOB '*[א-ת]*' AND length(t.transcript_json) > 100)))
          )
          AND (
               ? IS NULL
               OR (? = 'today' AND c.created_at >= date('now'))
               OR (? = '7d' AND c.created_at >= date('now', '-7 days'))
               OR (? = '30d' AND c.created_at >= date('now', '-30 days'))
          )
          AND (
               ? IS NULL
               OR c.external_id LIKE ?
               OR a.title LIKE ?
               OR t.transcript_json LIKE ?
          )
        ORDER BY c.is_favorite DESC, c.created_at DESC
        LIMIT ? OFFSET ?
    "#;

    let rows = sqlx::query(list_query)
        .bind(status_str)
        .bind(status_str)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(lang_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(date_filter)
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .bind(search_like.as_deref())
        .bind(page_size as i64)
        .bind(offset as i64)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let id_str: String = row.get("id");
        let org_id_str: String = row.get("organization_id");
        let external_id: Option<String> = row.get("external_id");
        let direction_str: String = row.get("direction");
        let phone_from: Option<String> = row.get("phone_from");
        let phone_to: Option<String> = row.get("phone_to");
        let started_at_str: Option<String> = row.get("started_at");
        let ended_at_str: Option<String> = row.get("ended_at");
        let duration_ms: Option<i64> = row.get("duration_ms");
        let status_str: String = row.get("processing_status");
        let is_favorite: bool = row
            .try_get::<i32, _>("is_favorite")
            .map(|v| v != 0)
            .unwrap_or(false);
        let tags_json: String = row
            .try_get::<String, _>("tags")
            .unwrap_or_else(|_| "[]".to_string());
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let created_at_str: String = row.get("created_at");
        let updated_at_str: String = row.get("updated_at");
        let sample_text: Option<String> = row.try_get("sample_text").ok();

        // Calculate language from text characters
        let detected_lang = if let Some(ref text) = sample_text {
            if text.chars().any(|c| ('\u{0590}'..='\u{05FF}').contains(&c)) {
                Some("he".to_string())
            } else if text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
                Some("ru".to_string())
            } else if !text.trim().is_empty() {
                Some("en".to_string())
            } else {
                None
            }
        } else {
            None
        };

        if let (Ok(id), Ok(organization_id)) =
            (CallId::from_str(&id_str), OrgId::from_str(&org_id_str))
        {
            let direction =
                CallDirection::from_str(&direction_str).unwrap_or(CallDirection::Unknown);
            let processing_status =
                ProcessingStatus::from_str(&status_str).unwrap_or(ProcessingStatus::Pending);
            let started_at = started_at_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            });
            let ended_at = ended_at_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            });
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let call = Call {
                id,
                organization_id,
                external_id,
                direction,
                phone_from,
                phone_to,
                started_at,
                ended_at,
                duration_ms: duration_ms.map(|d| d as u64),
                processing_status,
                is_favorite,
                tags,
                created_at,
                updated_at,
            };

            items.push(callmind_ui::CallListItem {
                call,
                language: detected_lang,
            });
        }
    }

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

    // Fetch analysis from database if available
    let analysis_row =
        sqlx::query("SELECT full_analysis_json FROM call_analyses WHERE call_id = ?")
            .bind(id.to_string())
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();

    let analysis: Option<callmind_analysis::CallAnalysis> = analysis_row.and_then(|r| {
        use sqlx::Row;
        let json_str: String = r.get("full_analysis_json");
        serde_json::from_str(&json_str).ok()
    });

    // Fetch transcript from database if available
    let transcript_row =
        sqlx::query("SELECT transcript_json FROM call_transcripts WHERE call_id = ?")
            .bind(id.to_string())
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();

    let transcript: Option<callmind_transcript::Transcript> = transcript_row.and_then(|r| {
        use sqlx::Row;
        let json_str: String = r.get("transcript_json");
        serde_json::from_str(&json_str).ok()
    });

    let last_error: Option<String> = sqlx::query_scalar(
        "SELECT last_error FROM jobs WHERE call_id = ? AND status = 'failed' ORDER BY created_at DESC LIMIT 1"
    )
    .bind(id.to_string())
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let html = render_call_detail(
        &call,
        transcript.as_ref(),
        analysis.as_ref(),
        last_error.as_deref(),
    );
    Ok(Html(html).into_response())
}

pub async fn analytics_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let row: (i64, Option<i64>, Option<f64>, Option<f64>) = sqlx::query_as(
        r#"SELECT count(*),
                  cast(sum(case when processing_status = 'completed' then 1 else 0 end) as integer),
                  cast(avg(duration_ms) as real),
                  cast(sum(duration_ms) as real)
           FROM calls"#,
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, None, None, None));

    let total_calls = row.0 as u64;
    let completed_calls = row.1.unwrap_or(0) as u64;
    let avg_duration_secs = row.2.unwrap_or(0.0) / 1000.0;
    let total_audio_hours = row.3.unwrap_or(0.0) / 3_600_000.0;

    // Top intents & topics from real database analysis
    let top_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT customer_intent, count(*)
           FROM call_analyses
           WHERE customer_intent IS NOT NULL AND customer_intent != ''
           GROUP BY customer_intent
           ORDER BY count(*) DESC
           LIMIT 5"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let total_analyzed = top_rows.iter().map(|(_, c)| *c).sum::<i64>().max(1) as f64;
    let top_intents = top_rows
        .into_iter()
        .map(|(intent, count)| {
            let pct = ((count as f64 / total_analyzed) * 100.0).round() as u32;
            (intent, count as u64, pct)
        })
        .collect();

    // Daily activity counts from real calls
    let daily_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT substr(created_at, 1, 10), count(*)
           FROM calls
           GROUP BY substr(created_at, 1, 10)
           ORDER BY substr(created_at, 1, 10) DESC
           LIMIT 7"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut daily_counts: Vec<(String, u64)> =
        daily_rows.into_iter().map(|(d, c)| (d, c as u64)).collect();
    daily_counts.reverse();

    // Real dynamic language distribution from transcripts
    let lang_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT
            CASE
                WHEN json_extract(transcript_json, '$.primary_language') IN ('he', 'Hebrew') THEN 'he'
                WHEN json_extract(transcript_json, '$.primary_language') IN ('ru', 'Russian') THEN 'ru'
                ELSE 'en'
            END as lang,
            count(*)
        FROM call_transcripts
        GROUP BY 1"#
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut hebrew_count = 0i64;
    let mut russian_count = 0i64;
    let mut english_count = 0i64;

    for (lang, count) in lang_rows {
        match lang.as_str() {
            "he" => hebrew_count += count,
            "ru" => russian_count += count,
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
