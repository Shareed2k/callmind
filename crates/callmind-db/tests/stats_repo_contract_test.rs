//! Dashboard aggregates, run against every backend by the same test.
//!
//! Aggregate return types are where backends differ most: Postgres types `avg()`
//! as `numeric` and `sum()` as `bigint`, where SQLite hands back floats. A wrong
//! read here does not error, it renders zeros — which is exactly how the earlier
//! analytics bug looked from the outside, so it is worth pinning.

mod backend;

use callmind_core::{EnqueueJob, JobKind};
use callmind_db::sql::{SqlJobRepository, SqlStatsRepository};
use callmind_db::{JobRepository, StatsRepository};

#[tokio::test]
async fn call_aggregates_match_on_every_backend() {
    for (name, conn) in backend::all("t_stats").await {
        // Two completed calls with durations, one pending without.
        backend::seed_call(&conn, "completed", Some(60_000)).await;
        backend::seed_call(&conn, "completed", Some(120_000)).await;
        backend::seed_call(&conn, "pending", None).await;

        let repo = SqlStatsRepository::new(conn.clone());
        let stats = repo.call_stats().await.expect("call_stats");

        assert_eq!(stats.total, 3, "{name}: total");
        assert_eq!(stats.completed, 2, "{name}: completed");
        // Averaged over the two non-NULL durations, not all three rows.
        assert!(
            (stats.avg_duration_ms - 90_000.0).abs() < 1.0,
            "{name}: avg was {}",
            stats.avg_duration_ms
        );
        assert!(
            (stats.total_duration_ms - 180_000.0).abs() < 1.0,
            "{name}: sum was {}",
            stats.total_duration_ms
        );

        let daily = repo.daily_call_counts(7).await.expect("daily");
        assert_eq!(daily, vec![("2026-08-23".to_string(), 3)], "{name}: daily");

        // Nothing analysed or transcribed yet: empty, not an error.
        assert!(
            repo.top_intents(5).await.expect("intents").is_empty(),
            "{name}"
        );
        assert!(
            repo.language_distribution()
                .await
                .expect("langs")
                .is_empty(),
            "{name}"
        );
    }
}

/// An empty database must give zeros rather than failing on NULL aggregates.
#[tokio::test]
async fn empty_database_reports_zeros() {
    for (name, conn) in backend::all("t_stats_empty").await {
        let stats = SqlStatsRepository::new(conn).call_stats().await.unwrap();
        assert_eq!(stats.total, 0, "{name}");
        assert_eq!(stats.completed, 0, "{name}");
        // Exactly zero: the point is that a NULL aggregate becomes 0.0 rather
        // than an error or a NaN, so an epsilon would defeat the assertion.
        assert!(stats.avg_duration_ms.abs() < f64::EPSILON, "{name}");
        assert!(stats.total_duration_ms.abs() < f64::EPSILON, "{name}");
    }
}

#[tokio::test]
async fn job_counts_and_last_error_match() {
    for (name, conn) in backend::all("t_stats_jobs").await {
        let call_id = backend::seed_call(&conn, "processing", None).await;
        let jobs = SqlJobRepository::new(conn.clone());
        let stats = SqlStatsRepository::new(conn.clone());

        assert!(
            stats.job_counts_by_status().await.unwrap().is_empty(),
            "{name}: counts before anything is queued"
        );

        let id = jobs
            .enqueue(
                &EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({}))
                    .with_call_id(callmind_core::CallId(call_id)),
            )
            .await
            .unwrap();
        assert_eq!(
            stats.job_counts_by_status().await.unwrap(),
            vec![("pending".to_string(), 1)],
            "{name}"
        );

        jobs.mark_failed(id, "gpu exploded", None).await.unwrap();
        assert_eq!(
            stats
                .last_job_error(callmind_core::CallId(call_id))
                .await
                .unwrap()
                .as_deref(),
            Some("gpu exploded"),
            "{name}"
        );
        assert_eq!(
            stats.job_counts_by_status().await.unwrap(),
            vec![("failed".to_string(), 1)],
            "{name}"
        );
    }
}

/// A call that failed once and then succeeded is not a failed call.
///
/// The detail page draws its "Processing Failed" banner from this method, so a
/// query that finds the last *failed* job rather than the *last* job leaves the
/// banner up forever: every reprocess adds a successful job without removing the
/// old failed one, and the user is told a call that completed did not.
#[tokio::test]
async fn a_later_success_clears_the_error() {
    for (name, conn) in backend::all("t_stats_recovered").await {
        let call_id = backend::seed_call(&conn, "processing", None).await;
        let jobs = SqlJobRepository::new(conn.clone());
        let stats = SqlStatsRepository::new(conn.clone());
        let call = callmind_core::CallId(call_id);

        let first = jobs
            .enqueue(
                &EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({}))
                    .with_call_id(call),
            )
            .await
            .unwrap();
        jobs.mark_failed(first, "gpu exploded", None).await.unwrap();
        assert_eq!(
            stats.last_job_error(call).await.unwrap().as_deref(),
            Some("gpu exploded"),
            "{name}: the failure is current until something newer says otherwise"
        );

        let second = jobs
            .enqueue(
                &EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({}))
                    .with_call_id(call),
            )
            .await
            .unwrap();
        jobs.mark_completed(second).await.unwrap();
        assert_eq!(
            stats.last_job_error(call).await.unwrap(),
            None,
            "{name}: the newest job succeeded, so there is no error to report"
        );

        // A job still running says nothing either way -- reporting the older
        // failure would put the banner back mid-reprocess.
        jobs.enqueue(
            &EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({})).with_call_id(call),
        )
        .await
        .unwrap();
        assert_eq!(
            stats.last_job_error(call).await.unwrap(),
            None,
            "{name}: a pending job is not a failure"
        );
    }
}
