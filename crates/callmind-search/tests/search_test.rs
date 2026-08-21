use callmind_core::{Call, CallDirection, OrgId};
use callmind_db::{CallRepository, SqliteCallRepository, create_sqlite_pool, run_migrations};
use callmind_llm::MockLlmEngine;
use callmind_search::{AskCallsRequest, AskEngine, IndexCallParams, SearchEngine, SearchFilter};
use std::sync::Arc;

#[tokio::test]
async fn test_fts5_multilingual_search_and_ask_pipeline() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("search_test.db");
    let pool = create_sqlite_pool(db_path.to_str().unwrap(), 5)
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let call_repo = SqliteCallRepository::new(pool.clone());
    let search_engine = SearchEngine::new(pool.clone());

    let org_id = OrgId::generate();
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?, ?, ?)")
        .bind(org_id.to_string())
        .bind("Test Search Org")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

    // 1. Create Call 1 (Hebrew - Cancellation)
    let call1 = Call::new(
        org_id,
        Some("pbx-he-1".into()),
        CallDirection::Incoming,
        Some("+972501112233".into()),
        Some("+97235550011".into()),
        Some(chrono::Utc::now()),
    );
    call_repo.create(&call1).await.unwrap();

    search_engine.index_call(IndexCallParams {
        call_id: call1.id,
        org_id,
        title: "בקשת ביטול מנוי",
        summary: "הלקוח התקשר וביקש לבצע ביטול מנוי עקב מעבר דירה. Customer requested subscription cancellation.",
        transcript: "שלום מדבר דני, אני מעוניין לבצע ביטול של המנוי שלי",
        topics: &["ביטול".into(), "אינטרנט".into(), "cancellation".into(), "subscription".into()],
        entities: &["Bezeq".into()],
        reason: Some("ביטול שירות"),
        resolution: Some("בוצע ביטול"),
    }).await.unwrap();

    // 2. Create Call 2 (Russian - Delivery Delay)
    let call2 = Call::new(
        org_id,
        Some("pbx-ru-2".into()),
        CallDirection::Incoming,
        Some("+972529998877".into()),
        Some("+97235550011".into()),
        Some(chrono::Utc::now()),
    );
    call_repo.create(&call2).await.unwrap();

    search_engine
        .index_call(IndexCallParams {
            call_id: call2.id,
            org_id,
            title: "Задержка доставки заказа",
            summary: "Клиент жалуется на задержку доставки курьером.",
            transcript: "Здравствуйте, моя доставка до сих пор не приехала уже три дня",
            topics: &["доставка".into(), "заказ".into()],
            entities: &["Wolt".into()],
            reason: Some("Задержка курьера"),
            resolution: Some("Назначен повторный выезд"),
        })
        .await
        .unwrap();

    // 3. Search in Hebrew
    let he_results = search_engine
        .search(&SearchFilter {
            query: "ביטול מנוי".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(he_results.len(), 1);
    assert_eq!(he_results[0].call_id, call1.id);
    assert!(
        he_results[0].match_highlight.contains("ביטול") || he_results[0].summary.contains("ביטול")
    );

    // 4. Search in Russian
    let ru_results = search_engine
        .search(&SearchFilter {
            query: "доставка курьером".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(ru_results.len(), 1);
    assert_eq!(ru_results[0].call_id, call2.id);

    // 5. Test AskEngine with Mock LLM
    let mock_llm_json = serde_json::json!({
        "answer": "Customers are canceling subscriptions primarily due to relocation, as seen in Source [1].",
        "cited_call_indices": [1]
    });

    let mock_llm = Arc::new(MockLlmEngine::new().with_json(mock_llm_json));
    let ask_engine = AskEngine::new(search_engine.clone(), mock_llm);

    let ask_res = ask_engine
        .ask(AskCallsRequest {
            question: "Why are customers requesting cancellation?".into(),
            organization_id: Some(org_id),
            max_sources: Some(5),
        })
        .await
        .unwrap();

    assert!(!ask_res.citations.is_empty());
    assert!(ask_res.answer.contains("relocation"));
    assert_eq!(ask_res.citations[0].call_id, call1.id);
}
