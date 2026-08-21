-- Transcripts Storage

CREATE TABLE IF NOT EXISTS call_transcripts (
    call_id TEXT PRIMARY KEY REFERENCES calls(id) ON DELETE CASCADE,
    transcript_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_transcripts_call_id ON call_transcripts(call_id);
