use crate::engine::SearchEngine;
use crate::errors::SearchError;
use crate::models::{AskCallsRequest, AskCallsResponse, CallCitation, SearchFilter};
use callmind_llm::LlmEngine;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmAskAnswer {
    answer: String,
    cited_call_indices: Option<Vec<usize>>,
}

/// AI analytical question answering over indexed call archives.
pub struct AskEngine {
    search: SearchEngine,
    llm: Arc<dyn LlmEngine>,
}

impl AskEngine {
    pub fn new(search: SearchEngine, llm: Arc<dyn LlmEngine>) -> Self {
        Self { search, llm }
    }

    /// Answer analytical questions across calls with evidence citations.
    pub async fn ask(&self, req: AskCallsRequest) -> Result<AskCallsResponse, SearchError> {
        let max_sources = req.max_sources.unwrap_or(5);

        // 1. Search top matching calls for the question
        let filter = SearchFilter {
            organization_id: req.organization_id,
            query: req.question.clone(),
            limit: Some(max_sources as u32),
            ..Default::default()
        };

        let hits = self.search.search(&filter).await?;

        if hits.is_empty() {
            return Ok(AskCallsResponse {
                answer: "No relevant conversations found matching the query in the call archives."
                    .into(),
                citations: Vec::new(),
            });
        }

        // 2. Format search hits as ground-truth context
        let mut sources_context = String::new();
        let mut candidate_citations = Vec::new();

        for (idx, hit) in hits.into_iter().enumerate() {
            let _ = write!(
                sources_context,
                "Source [{}]: Call ID: {}\nTitle: {}\nSummary: {}\nExcerpt: {}\n\n",
                idx + 1,
                hit.call_id,
                hit.title,
                hit.summary,
                hit.match_highlight
            );

            candidate_citations.push(CallCitation {
                call_id: hit.call_id,
                text_snippet: hit.summary,
                relevance_score: 0.90,
            });
        }

        // 3. Prompt LLM to answer using evidence
        let system_prompt = r#"
You are a conversation intelligence analyst. Answer the user's analytical question based ONLY on the provided conversation sources.
Cite specific source numbers (e.g. Source [1], Source [2]) in your answer.
Do not hallucinate facts outside the provided sources.
"#;

        let prompt = format!(
            r#"
Question: "{}"

Available Call Sources:
---
{}
---

Return JSON:
{{
  "answer": "synthesized response citing sources",
  "cited_call_indices": [1, 2]
}}
"#,
            req.question, sources_context
        );

        let llm_res: Result<LlmAskAnswer, _> = self
            .llm
            .generate_structured(&prompt, Some(system_prompt))
            .await;

        let (answer, citations) = match llm_res {
            Ok(parsed) => {
                let filtered_citations = if let Some(indices) = parsed.cited_call_indices {
                    let mut selected = Vec::new();
                    for idx in indices {
                        // Indices in prompt are 1-based (Source [1], Source [2], ...)
                        let array_idx = if idx > 0 { idx - 1 } else { idx };
                        if let Some(cit) = candidate_citations.get(array_idx) {
                            if !selected
                                .iter()
                                .any(|c: &CallCitation| c.call_id == cit.call_id)
                            {
                                selected.push(cit.clone());
                            }
                        }
                    }
                    if selected.is_empty() {
                        candidate_citations
                    } else {
                        selected
                    }
                } else {
                    candidate_citations
                };
                (parsed.answer, filtered_citations)
            }
            Err(_) => (
                format!(
                    "Found {} relevant call(s) regarding your query. Please review the cited calls below.",
                    candidate_citations.len()
                ),
                candidate_citations,
            ),
        };

        Ok(AskCallsResponse { answer, citations })
    }
}
