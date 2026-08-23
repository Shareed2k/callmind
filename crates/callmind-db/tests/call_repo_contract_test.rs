//! The `CallRepository` contract, run against every backend by one test body.
//!
//! This is the repository with the most places to diverge: an upsert, a
//! transaction, a self-referencing `UPDATE`, JSON extraction, a `NOT IN`
//! subquery and full-text search. Each of those is spelled differently enough
//! between SQLite and Postgres that "it compiles" says almost nothing.

mod backend;

use callmind_core::{
    Call, CallDirection, CallFilter, CallId, EnqueueJob, JobKind, OrgId, ProcessingStatus,
    Recording, RecordingId,
};
use callmind_db::sql::{SqlCallRepository, SqlJobRepository};
use callmind_db::{AnalysisRow, CallListFilter, CallRepository, JobRepository};

fn call(external_id: &str) -> Call {
    let mut c = Call::new(
        OrgId::DEFAULT,
        Some(external_id.to_string()),
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    c.duration_ms = Some(90_000);
    c
}

#[tokio::test]
async fn call_crud_round_trips_on_every_backend() {
    for (name, conn) in backend::all("t_call_crud").await {
        let repo = SqlCallRepository::new(conn);

        let mut original = call("ext-1");
        original.phone_from = Some("+972500000000".to_string());
        repo.create(&original).await.expect("create");

        // `(organization_id, external_id)` is unique, and the second insert has
        // to come back classified as a duplicate rather than as a generic query
        // failure: the importer keys its skip-already-imported decision on that
        // distinction, and the two backends word the error differently.
        let dup = Call {
            id: CallId::generate(),
            ..original.clone()
        };
        assert!(
            matches!(
                repo.create(&dup).await,
                Err(callmind_db::DbError::DuplicateKey(_))
            ),
            "{name}: re-importing the same external id must report a duplicate"
        );

        let fetched = repo
            .get_by_id(original.id)
            .await
            .expect("get_by_id")
            .expect("row exists");
        assert_eq!(fetched.external_id.as_deref(), Some("ext-1"), "{name}");
        assert_eq!(fetched.duration_ms, Some(90_000), "{name}");
        assert_eq!(fetched.direction, CallDirection::Incoming, "{name}");
        assert_eq!(
            fetched.phone_from.as_deref(),
            Some("+972500000000"),
            "{name}"
        );
        assert!(!fetched.is_favorite, "{name}: default");
        assert!(fetched.tags.is_empty(), "{name}: default");

        assert!(
            repo.get_by_external_id(OrgId::DEFAULT, "ext-1")
                .await
                .expect("by external id")
                .is_some(),
            "{name}"
        );
        assert!(
            repo.get_by_external_id(OrgId::DEFAULT, "nope")
                .await
                .expect("by external id")
                .is_none(),
            "{name}"
        );

        // Favourite is flipped in SQL, so the toggle has to report the value it
        // actually wrote.
        assert!(
            repo.toggle_favorite(original.id).await.expect("on"),
            "{name}"
        );
        assert!(
            !repo.toggle_favorite(original.id).await.expect("off"),
            "{name}"
        );
        assert!(
            repo.toggle_favorite(CallId::generate()).await.is_err(),
            "{name}: toggling a missing call must fail, not report false"
        );

        repo.update_tags(original.id, &["family".into(), "urgent".into()])
            .await
            .expect("tags");
        assert_eq!(
            repo.get_by_id(original.id).await.unwrap().unwrap().tags,
            vec!["family".to_string(), "urgent".to_string()],
            "{name}"
        );
        assert!(
            repo.update_tags(CallId::generate(), &[]).await.is_err(),
            "{name}: tagging a missing call must fail"
        );

        repo.update_status(original.id, ProcessingStatus::Completed)
            .await
            .expect("status");
        assert_eq!(
            repo.get_by_id(original.id)
                .await
                .unwrap()
                .unwrap()
                .processing_status,
            ProcessingStatus::Completed,
            "{name}"
        );

        assert_eq!(
            repo.get_organization_name(OrgId::DEFAULT).await.unwrap(),
            Some("Default Organization".to_string()),
            "{name}: the migration seeds this"
        );

        assert!(repo.delete(original.id).await.expect("delete"), "{name}");
        assert!(
            !repo.delete(original.id).await.expect("delete again"),
            "{name}"
        );
    }
}

#[tokio::test]
async fn list_filters_and_orders_the_same_way() {
    for (name, conn) in backend::all("t_call_list").await {
        let repo = SqlCallRepository::new(conn);

        for i in 0..3 {
            let mut c = call(&format!("ext-{i}"));
            c.processing_status = if i == 0 {
                ProcessingStatus::Completed
            } else {
                ProcessingStatus::Pending
            };
            repo.create(&c).await.unwrap();
            if i == 2 {
                repo.toggle_favorite(c.id).await.unwrap();
            }
        }

        let all = repo.list(&CallFilter::default()).await.expect("list");
        assert_eq!(all.len(), 3, "{name}");
        // Favourites first, which the calls list depends on.
        assert!(all[0].is_favorite, "{name}: favourite must sort first");

        let completed = repo
            .list(&CallFilter {
                status: Some(ProcessingStatus::Completed),
                ..Default::default()
            })
            .await
            .expect("filtered");
        assert_eq!(completed.len(), 1, "{name}");

        let page = repo
            .list(&CallFilter {
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            })
            .await
            .expect("paged");
        assert_eq!(page.len(), 1, "{name}: limit and offset");

        assert!(
            repo.list(&CallFilter {
                external_id: Some("ext-1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap()
            .len()
                == 1,
            "{name}"
        );
    }
}

/// Transcripts and analyses: the upsert and the transaction.
#[tokio::test]
async fn transcript_and_analysis_survive_a_rewrite() {
    for (name, conn) in backend::all("t_call_analysis").await {
        let repo = SqlCallRepository::new(conn);
        let c = call("ext-analysis");
        repo.create(&c).await.unwrap();

        assert!(
            repo.get_transcript_json(c.id).await.unwrap().is_none(),
            "{name}: nothing stored yet"
        );

        repo.save_transcript(c.id, r#"{"languages":[{"language":"hebrew"}]}"#)
            .await
            .expect("first save");
        // Saving twice must upsert, not fail on the primary key -- a retried job
        // does exactly this.
        repo.save_transcript(c.id, r#"{"languages":[{"language":"russian"}]}"#)
            .await
            .expect("second save must upsert");
        assert!(
            repo.get_transcript_json(c.id)
                .await
                .unwrap()
                .unwrap()
                .contains("russian"),
            "{name}"
        );

        repo.update_transcript_json(
            c.id,
            r#"{"languages":[{"language":"hebrew"}],"edited":true}"#,
        )
        .await
        .expect("rename a speaker");
        assert!(
            repo.get_transcript_json(c.id)
                .await
                .unwrap()
                .unwrap()
                .contains("edited"),
            "{name}"
        );

        assert!(
            repo.get_analysis_json(c.id).await.unwrap().is_none(),
            "{name}"
        );
        for title in ["first pass", "second pass"] {
            repo.commit_analysis(
                &AnalysisRow {
                    id: uuid::Uuid::new_v4(),
                    call_id: c.id,
                    title,
                    summary: "summary",
                    reason: None,
                    resolution: None,
                    resolved: true,
                    customer_intent: Some("question"),
                    sentiment_score: 0.5,
                    metrics_json: "{}",
                    full_analysis_json: r#"{"ok":true}"#,
                    created_at: chrono::Utc::now(),
                },
                ProcessingStatus::Completed,
            )
            .await
            .expect("commit_analysis");
        }
        let (title, json) = repo.get_analysis_json(c.id).await.unwrap().unwrap();
        assert_eq!(title, "second pass", "{name}: re-analysis replaces");
        assert_eq!(json, r#"{"ok":true}"#, "{name}");
        // The status update rides in the same transaction as the analysis.
        assert_eq!(
            repo.get_by_id(c.id)
                .await
                .unwrap()
                .unwrap()
                .processing_status,
            ProcessingStatus::Completed,
            "{name}"
        );

        repo.update_analysis_title(c.id, "renamed").await.unwrap();
        assert_eq!(
            repo.get_analysis_json(c.id).await.unwrap().unwrap().0,
            "renamed",
            "{name}"
        );

        // Plugin results share one table, upserted per (call, plugin).
        repo.save_plugin_result(c.id, "emotions", r#"{"v":1}"#)
            .await
            .unwrap();
        repo.save_plugin_result(c.id, "emotions", r#"{"v":2}"#)
            .await
            .expect("must upsert on the composite key");
        repo.save_plugin_result(c.id, "biometrics", "{}")
            .await
            .unwrap();
        let results = repo.list_plugin_results(c.id).await.unwrap();
        assert_eq!(
            results,
            vec![
                ("biometrics".to_string(), "{}".to_string()),
                ("emotions".to_string(), r#"{"v":2}"#.to_string()),
            ],
            "{name}"
        );
    }
}

#[tokio::test]
async fn recordings_round_trip() {
    for (name, conn) in backend::all("t_call_recording").await {
        let repo = SqlCallRepository::new(conn);
        let c = call("ext-rec");
        repo.create(&c).await.unwrap();

        assert!(
            repo.get_recording_by_call_id(c.id).await.unwrap().is_none(),
            "{name}"
        );

        let rec = Recording {
            id: RecordingId::generate(),
            call_id: c.id,
            storage_key: "calls/rec.m4a".to_string(),
            mime_type: "audio/mp4".to_string(),
            file_size_bytes: 41_600_000,
            sha256: "abc123".to_string(),
            duration_ms: Some(2_574_000),
            channels: Some(2),
            sample_rate: Some(48_000),
            created_at: chrono::Utc::now(),
        };
        repo.add_recording(&rec).await.expect("add");

        let got = repo
            .get_recording_by_call_id(c.id)
            .await
            .unwrap()
            .expect("stored");
        assert_eq!(got.storage_key, "calls/rec.m4a", "{name}");
        // A 43-minute recording is past i32, so the width has to survive.
        assert_eq!(got.file_size_bytes, 41_600_000, "{name}");
        assert_eq!(got.duration_ms, Some(2_574_000), "{name}");
        assert_eq!(got.channels, Some(2), "{name}");
        assert_eq!(got.sample_rate, Some(48_000), "{name}");

        // Audio metadata is written to the call and its recording together.
        // Before this existed, `calls.duration_ms` was NULL for everything that
        // did not come through the batch importer, which showed as a dash in the
        // calls list and a zero in the analytics averages.
        repo.set_audio_metadata(c.id, 2_574_321, 1, 16_000)
            .await
            .expect("set_audio_metadata");
        assert_eq!(
            repo.get_by_id(c.id).await.unwrap().unwrap().duration_ms,
            Some(2_574_321),
            "{name}: the call carries the duration"
        );
        let updated = repo
            .get_recording_by_call_id(c.id)
            .await
            .unwrap()
            .expect("stored");
        assert_eq!(updated.duration_ms, Some(2_574_321), "{name}");
        assert_eq!(updated.channels, Some(1), "{name}: mono after decoding");
        assert_eq!(updated.sample_rate, Some(16_000), "{name}");

        // A call with no recording row yet must still take the duration rather
        // than failing: some ingestion paths create the two separately.
        let bare = call("ext-no-recording");
        repo.create(&bare).await.unwrap();
        repo.set_audio_metadata(bare.id, 5_000, 2, 48_000)
            .await
            .expect("no recording row is not an error");
        assert_eq!(
            repo.get_by_id(bare.id).await.unwrap().unwrap().duration_ms,
            Some(5_000),
            "{name}"
        );

        assert!(repo.delete_recording(rec.id).await.unwrap(), "{name}");
        assert!(!repo.delete_recording(rec.id).await.unwrap(), "{name}");
    }
}

/// The startup reconciliation sweeps, which share one statement.
#[tokio::test]
async fn orphaned_processing_calls_are_reconciled() {
    for (name, conn) in backend::all("t_call_sweep").await {
        let calls = SqlCallRepository::new(conn.clone());
        let jobs = SqlJobRepository::new(conn);

        // Covered by a pending job: belongs back in the queue.
        let covered = call("ext-covered");
        calls.create(&covered).await.unwrap();
        calls
            .update_status(covered.id, ProcessingStatus::Processing)
            .await
            .unwrap();
        jobs.enqueue(
            &EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({}))
                .with_call_id(covered.id),
        )
        .await
        .unwrap();

        // No job at all: the process died before enqueueing, so this can only fail.
        let orphan = call("ext-orphan");
        calls.create(&orphan).await.unwrap();
        calls
            .update_status(orphan.id, ProcessingStatus::Processing)
            .await
            .unwrap();

        assert_eq!(
            calls.reset_processing_to_pending().await.unwrap(),
            1,
            "{name}: only the covered call goes back to pending"
        );
        assert_eq!(
            calls
                .get_by_id(covered.id)
                .await
                .unwrap()
                .unwrap()
                .processing_status,
            ProcessingStatus::Pending,
            "{name}"
        );

        assert_eq!(
            calls.fail_orphaned_processing().await.unwrap(),
            1,
            "{name}: only the uncovered call fails"
        );
        assert_eq!(
            calls
                .get_by_id(orphan.id)
                .await
                .unwrap()
                .unwrap()
                .processing_status,
            ProcessingStatus::Failed,
            "{name}"
        );
    }
}

/// `list_for_display` is the read model behind the calls page: a join, JSON
/// extraction, the language column and full-text search, all in one statement.
#[tokio::test]
async fn display_list_filters_match_across_backends() {
    for (name, conn) in backend::all("t_call_display").await {
        let repo = SqlCallRepository::new(conn);

        let hebrew = call("ext-he");
        repo.create(&hebrew).await.unwrap();
        repo.save_transcript(
            hebrew.id,
            r#"{"languages":[{"language":"hebrew"}],
                "segments":[{"normalized_text":"שלום, מה קורה"}]}"#,
        )
        .await
        .unwrap();

        let russian = call("ext-ru");
        repo.create(&russian).await.unwrap();
        repo.update_status(russian.id, ProcessingStatus::Completed)
            .await
            .unwrap();
        repo.save_transcript(
            russian.id,
            r#"{"languages":[{"language":"russian"}],
                "segments":[{"normalized_text":"привет, как дела"}]}"#,
        )
        .await
        .unwrap();

        let base = CallListFilter {
            status: None,
            language: None,
            date: None,
            search: None,
            limit: 25,
            offset: 0,
        };

        let (rows, total) = repo.list_for_display(&base).await.expect("unfiltered");
        assert_eq!((rows.len(), total), (2, 2), "{name}");
        // The preview text comes out of the transcript JSON, which is the one
        // expression with no portable spelling.
        let previews: Vec<_> = rows.iter().filter_map(|r| r.sample_text.clone()).collect();
        assert_eq!(
            previews.len(),
            2,
            "{name}: sample_text extracted: {previews:?}"
        );
        assert!(
            previews.iter().any(|p| p.contains("שלום")),
            "{name}: {previews:?}"
        );
        // Read from the generated column, not sniffed from characters: the badge
        // and the filter have to agree.
        assert!(
            rows.iter()
                .any(|r| r.primary_language.as_deref() == Some("hebrew")),
            "{name}"
        );

        for (code, expected) in [("he", 1), ("ru", 1), ("en", 0)] {
            let (rows, total) = repo
                .list_for_display(&CallListFilter {
                    language: Some(code),
                    ..base
                })
                .await
                .expect("language filter");
            assert_eq!(
                (rows.len(), total as usize),
                (expected, expected),
                "{name}: {code}"
            );
        }

        let (_, completed) = repo
            .list_for_display(&CallListFilter {
                status: Some("completed"),
                ..base
            })
            .await
            .unwrap();
        assert_eq!(completed, 1, "{name}: status filter");

        let (_, recent) = repo
            .list_for_display(&CallListFilter {
                date: Some("today"),
                ..base
            })
            .await
            .unwrap();
        assert_eq!(recent, 2, "{name}: both were created just now");

        // The external id is matched literally, because a call identifier is not
        // natural language and the tokenizer would split it.
        let (_, by_id) = repo
            .list_for_display(&CallListFilter {
                search: Some("ext-he"),
                ..base
            })
            .await
            .unwrap();
        assert_eq!(by_id, 1, "{name}: external id search");

        let (_, nothing) = repo
            .list_for_display(&CallListFilter {
                search: Some("zzzznomatch"),
                ..base
            })
            .await
            .unwrap();
        assert_eq!(
            nothing, 0,
            "{name}: a term nothing holds must return nothing"
        );

        // Pagination is applied to the page but not to the total.
        let (page, total) = repo
            .list_for_display(&CallListFilter { limit: 1, ..base })
            .await
            .unwrap();
        assert_eq!((page.len(), total), (1, 2), "{name}");
    }
}
