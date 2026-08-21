-- FTS5 Full-Text Search and Call Analyses

CREATE TABLE IF NOT EXISTS call_analyses (
    id TEXT PRIMARY KEY,
    call_id TEXT NOT NULL UNIQUE REFERENCES calls(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    reason TEXT,
    resolution TEXT,
    resolved INTEGER NOT NULL DEFAULT 1,
    customer_intent TEXT,
    sentiment_score REAL NOT NULL DEFAULT 0.0,
    metrics_json TEXT NOT NULL,
    full_analysis_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_analyses_call_id ON call_analyses(call_id);
CREATE INDEX IF NOT EXISTS idx_analyses_sentiment ON call_analyses(sentiment_score);
CREATE INDEX IF NOT EXISTS idx_analyses_resolved ON call_analyses(resolved);

-- Virtual table for multilingual Full-Text Search with Unicode61 tokenizer
CREATE VIRTUAL TABLE IF NOT EXISTS fts_calls USING fts5(
    call_id UNINDEXED,
    organization_id UNINDEXED,
    title,
    summary,
    transcript,
    topics,
    entities,
    reason,
    resolution,
    tokenize = 'unicode61 remove_diacritics 2'
);
