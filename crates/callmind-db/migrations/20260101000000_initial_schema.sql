-- Initial schema for CallMind

CREATE TABLE IF NOT EXISTS organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Insert default organization
INSERT OR IGNORE INTO organizations (id, name, created_at)
VALUES ('00000000-0000-0000-0000-000000000001', 'Default Organization', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE IF NOT EXISTS calls (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    external_id TEXT,
    direction TEXT NOT NULL,
    phone_from TEXT,
    phone_to TEXT,
    started_at TEXT,
    ended_at TEXT,
    duration_ms INTEGER,
    processing_status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_calls_org_created ON calls(organization_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_calls_external_id ON calls(organization_id, external_id);
CREATE INDEX IF NOT EXISTS idx_calls_status ON calls(processing_status);

CREATE TABLE IF NOT EXISTS call_recordings (
    id TEXT PRIMARY KEY,
    call_id TEXT NOT NULL UNIQUE REFERENCES calls(id) ON DELETE CASCADE,
    storage_key TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    file_size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    duration_ms INTEGER,
    channels INTEGER,
    sample_rate INTEGER,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recordings_call_id ON call_recordings(call_id);
CREATE INDEX IF NOT EXISTS idx_recordings_sha256 ON call_recordings(sha256);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    call_id TEXT REFERENCES calls(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    attempt INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    run_after TEXT NOT NULL,
    locked_at TEXT,
    locked_by TEXT,
    last_error TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_jobs_fetch ON jobs(status, run_after, priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_jobs_call_id ON jobs(call_id);
