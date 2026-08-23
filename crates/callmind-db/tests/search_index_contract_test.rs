//! The `SearchIndex` contract, run against FTS5 and `tsvector` by one test body.
//!
//! Unlike the other repositories, the two backends here are not the same query
//! spelled differently -- they are different engines with different operators,
//! different scoring functions and opposite sort directions. So the assertions
//! are on observable behaviour: what matches, what does not, what ranks first,
//! and what the highlight contains.

mod backend;

use callmind_core::{Call, CallDirection, OrgId};
use callmind_db::sql::{SqlCallRepository, SqlSearchIndex};
use callmind_db::{CallRepository, IndexDocument, SearchIndex, SearchQuery};

fn query(text: &str) -> SearchQuery<'_> {
    SearchQuery {
        text,
        organization_id: None,
        from_date: None,
        to_date: None,
        direction: None,
        status: None,
        limit: 20,
        offset: 0,
    }
}

/// Seed a call and index it, returning its id.
async fn indexed(
    calls: &SqlCallRepository,
    index: &SqlSearchIndex,
    title: &str,
    summary: &str,
    transcript: &str,
) -> callmind_core::CallId {
    let call = Call::new(
        OrgId::DEFAULT,
        None,
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    calls.create(&call).await.expect("create call");
    index
        .index(&IndexDocument {
            call_id: call.id,
            org_id: OrgId::DEFAULT,
            title,
            summary,
            transcript,
            topics: &["logistics".to_string()],
            entities: &["Dana".to_string()],
            reason: Some("delivery delay"),
            resolution: None,
        })
        .await
        .expect("index");
    call.id
}

#[tokio::test]
async fn search_finds_the_same_calls_on_every_engine() {
    for (name, conn) in backend::all("t_search").await {
        let calls = SqlCallRepository::new(conn.clone());
        let index = SqlSearchIndex::new(conn);

        let ru = indexed(
            &calls,
            &index,
            "Телефонный разговор",
            "Обсуждение отмены заказа",
            "клиент просит отменить заказ на счет доставки",
        )
        .await;
        let he = indexed(
            &calls,
            &index,
            "השיחה",
            "בקשה לביטול הזמנה",
            "הלקוח ביקש לבטל את ההזמנה",
        )
        .await;

        // Russian inflects by suffix, which a prefix term covers.
        let hits = index.search(&query("разгов")).await.expect("search");
        assert_eq!(hits.len(), 1, "{name}: prefix term");
        assert_eq!(hits[0].call_id, ru, "{name}");

        // The index covers the summary too, not just the transcript -- the
        // `LIKE` this replaced searched neither.
        assert_eq!(
            index.search(&query("отмены")).await.unwrap().len(),
            1,
            "{name}: summary is indexed"
        );
        assert_eq!(
            index.search(&query("logistics")).await.unwrap().len(),
            2,
            "{name}: topics are indexed"
        );
        assert_eq!(
            index.search(&query("delivery")).await.unwrap().len(),
            2,
            "{name}: reason is indexed"
        );

        // The Hebrew case that broke when this moved off `LIKE`: the stored title
        // is `השיחה` and the user types the bare stem. No full-text engine's
        // prefix term matches that on its own, so the proclitic expansion has to
        // be carrying it -- on both backends.
        let bare = index.search(&query("שיחה")).await.expect("bare stem");
        assert_eq!(bare.len(), 1, "{name}: bare Hebrew stem must match `השיחה`");
        assert_eq!(bare[0].call_id, he, "{name}");
        assert_eq!(
            index.search(&query("השיחה")).await.unwrap().len(),
            1,
            "{name}: the prefixed form matches too"
        );

        // Nothing, and a query that expands to no terms at all.
        assert!(
            index
                .search(&query("zzzznomatch"))
                .await
                .unwrap()
                .is_empty(),
            "{name}"
        );
        assert!(
            index.search(&query("   ")).await.unwrap().is_empty(),
            "{name}"
        );
        assert!(
            index.search(&query("\"*:()")).await.unwrap().is_empty(),
            "{name}: operator-only input must not error"
        );
    }
}

/// Scoring and the snippet, the two places the engines share no code at all.
#[tokio::test]
async fn ranking_puts_the_better_match_first() {
    for (name, conn) in backend::all("t_search_rank").await {
        let calls = SqlCallRepository::new(conn.clone());
        let index = SqlSearchIndex::new(conn);

        // One passing mention, versus a call that is about the term.
        let passing = indexed(&calls, &index, "unrelated", "unrelated", "доставка").await;
        let focused = indexed(
            &calls,
            &index,
            "доставка",
            "доставка задерживается",
            "доставка доставка доставка задержка доставки",
        )
        .await;

        let hits = index.search(&query("доставка")).await.expect("search");
        assert_eq!(hits.len(), 2, "{name}");
        // `bm25()` sorts ascending and `ts_rank` descending; if either direction
        // were wrong this would return the worst match first, which looks like a
        // broken index rather than a bug.
        assert_eq!(
            hits[0].call_id, focused,
            "{name}: the denser match must rank first"
        );
        assert_eq!(hits[1].call_id, passing, "{name}");

        // The highlight is `snippet()` on one backend and `ts_headline()` on the
        // other; both must mark the hit up the same way, because the UI renders
        // the tags.
        assert!(
            hits[0].match_highlight.contains("<b>"),
            "{name}: highlight was {:?}",
            hits[0].match_highlight
        );
        assert!(
            hits[0].match_highlight.to_lowercase().contains("доставка"),
            "{name}: highlight was {:?}",
            hits[0].match_highlight
        );
    }
}

#[tokio::test]
async fn reindex_replaces_and_delete_removes() {
    for (name, conn) in backend::all("t_search_write").await {
        let calls = SqlCallRepository::new(conn.clone());
        let index = SqlSearchIndex::new(conn);

        let id = indexed(&calls, &index, "before", "first summary", "первый текст").await;
        assert_eq!(
            index.search(&query("before")).await.unwrap().len(),
            1,
            "{name}"
        );

        // Re-indexing the same call must replace its row, not add a second one.
        index
            .index(&IndexDocument {
                call_id: id,
                org_id: OrgId::DEFAULT,
                title: "after",
                summary: "second summary",
                transcript: "второй текст",
                topics: &[],
                entities: &[],
                reason: None,
                resolution: None,
            })
            .await
            .expect("reindex");
        assert!(
            index.search(&query("before")).await.unwrap().is_empty(),
            "{name}: the old row must be gone"
        );
        assert_eq!(
            index.search(&query("after")).await.unwrap().len(),
            1,
            "{name}"
        );

        // A rename that does not re-run analysis.
        index
            .update_title(id, "renamed")
            .await
            .expect("update_title");
        let hits = index.search(&query("renamed")).await.unwrap();
        assert_eq!(hits.len(), 1, "{name}: the title change must be searchable");
        assert_eq!(hits[0].title, "renamed", "{name}");
        // On Postgres the `tsvector` is a generated column, so this also proves
        // it re-derived itself from the updated title.
        assert_eq!(
            index.search(&query("after")).await.unwrap().len(),
            0,
            "{name}: the old title must stop matching"
        );

        index.delete(id).await.expect("delete");
        assert!(
            index.search(&query("renamed")).await.unwrap().is_empty(),
            "{name}"
        );
        // Deleting again is not an error.
        index.delete(id).await.expect("idempotent delete");
    }
}

/// The metadata filters, which sit alongside the match on both backends.
#[tokio::test]
async fn metadata_filters_narrow_the_match() {
    for (name, conn) in backend::all("t_search_filter").await {
        let calls = SqlCallRepository::new(conn.clone());
        let index = SqlSearchIndex::new(conn);

        let id = indexed(&calls, &index, "заказ", "заказ", "заказ доставки").await;
        indexed(&calls, &index, "заказ", "заказ", "заказ доставки").await;

        assert_eq!(
            index.search(&query("заказ")).await.unwrap().len(),
            2,
            "{name}"
        );

        assert_eq!(
            index
                .search(&SearchQuery {
                    organization_id: Some(OrgId::DEFAULT),
                    ..query("заказ")
                })
                .await
                .unwrap()
                .len(),
            2,
            "{name}: org filter"
        );

        calls
            .update_status(id, callmind_core::ProcessingStatus::Completed)
            .await
            .unwrap();
        assert_eq!(
            index
                .search(&SearchQuery {
                    status: Some("completed"),
                    ..query("заказ")
                })
                .await
                .unwrap()
                .len(),
            1,
            "{name}: status filter joins through to `calls`"
        );
        assert_eq!(
            index
                .search(&SearchQuery {
                    direction: Some("outgoing"),
                    ..query("заказ")
                })
                .await
                .unwrap()
                .len(),
            0,
            "{name}: direction filter"
        );

        // Paging applies to the match, ordered consistently.
        let page = index
            .search(&SearchQuery {
                limit: 1,
                ..query("заказ")
            })
            .await
            .unwrap();
        assert_eq!(page.len(), 1, "{name}");
        let next = index
            .search(&SearchQuery {
                limit: 1,
                offset: 1,
                ..query("заказ")
            })
            .await
            .unwrap();
        assert_eq!(next.len(), 1, "{name}");
        assert_ne!(
            page[0].call_id, next[0].call_id,
            "{name}: pages must not overlap"
        );
    }
}
