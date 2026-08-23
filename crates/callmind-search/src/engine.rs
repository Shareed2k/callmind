use crate::errors::SearchError;
use crate::models::{SearchFilter, SearchResultItem};
use callmind_core::{CallId, OrgId};
use callmind_db::{IndexDocument, SearchIndex, SearchQuery};
use std::sync::Arc;

/// Input payload for indexing a conversation in the search index.
pub struct IndexCallParams<'a> {
    pub call_id: CallId,
    pub org_id: OrgId,
    pub title: &'a str,
    pub summary: &'a str,
    pub transcript: &'a str,
    pub topics: &'a [String],
    pub entities: &'a [String],
    pub reason: Option<&'a str>,
    pub resolution: Option<&'a str>,
}

/// Full-text search over conversations.
///
/// Holds no SQL of its own: the index lives behind
/// [`callmind_db::SearchIndex`], which is what lets the same search work over
/// SQLite FTS5 and a Postgres `tsvector`. What stays here is the API-facing
/// shape — the request filter and the result item the HTTP layer serialises.
#[derive(Clone)]
pub struct SearchEngine {
    index: Arc<dyn SearchIndex>,
}

impl std::fmt::Debug for SearchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchEngine").finish_non_exhaustive()
    }
}

impl SearchEngine {
    pub fn new(index: Arc<dyn SearchIndex>) -> Self {
        Self { index }
    }

    /// Index or update a call in the search index.
    pub async fn index_call(&self, params: IndexCallParams<'_>) -> Result<(), SearchError> {
        self.index
            .index(&IndexDocument {
                call_id: params.call_id,
                org_id: params.org_id,
                title: params.title,
                summary: params.summary,
                transcript: params.transcript,
                topics: params.topics,
                entities: params.entities,
                reason: params.reason,
                resolution: params.resolution,
            })
            .await?;
        Ok(())
    }

    /// Update just the indexed title, for a rename that does not re-analyse.
    pub async fn update_indexed_title(
        &self,
        call_id: CallId,
        title: &str,
    ) -> Result<(), SearchError> {
        self.index.update_title(call_id, title).await?;
        Ok(())
    }

    /// Search indexed calls by free text plus metadata filters.
    pub async fn search(
        &self,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchResultItem>, SearchError> {
        let text = filter.query.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let hits = self
            .index
            .search(&SearchQuery {
                text,
                organization_id: filter.organization_id,
                from_date: filter.from_date,
                to_date: filter.to_date,
                direction: filter.direction.map(|d| d.as_str()),
                status: filter.status.map(|s| s.as_str()),
                limit: filter.limit.unwrap_or(20),
                offset: filter.offset.unwrap_or(0),
            })
            .await?;

        Ok(hits
            .into_iter()
            .map(|hit| SearchResultItem {
                call_id: hit.call_id,
                title: hit.title,
                summary: hit.summary,
                match_highlight: hit.match_highlight,
                rank: hit.rank,
                created_at: hit.created_at,
            })
            .collect())
    }

    /// Remove a call from the search index.
    pub async fn delete_index(&self, call_id: CallId) -> Result<(), SearchError> {
        self.index.delete(call_id).await?;
        Ok(())
    }
}

/// Escape a free-text query for FTS5.
///
/// A thin alias over [`callmind_core::search_query::to_fts5`]; the expansion
/// rules live in `callmind-core` so the Postgres `tsvector` path shares them.
#[must_use]
pub fn sanitize_fts5_query(query: &str) -> String {
    callmind_core::search_query::to_fts5(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules themselves are tested in `callmind-core`; this only pins that
    /// the search engine still routes through them.
    #[test]
    fn sanitize_delegates_to_the_shared_expansion() {
        assert_eq!(
            sanitize_fts5_query("отмена заказа"),
            "\"отмена\"* OR \"заказа\"*"
        );
        assert!(sanitize_fts5_query("שיחה").contains("\"השיחה\"*"));
        assert_eq!(sanitize_fts5_query("   "), "");
    }
}
