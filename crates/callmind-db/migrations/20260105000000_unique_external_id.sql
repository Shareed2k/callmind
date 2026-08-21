-- Ensure uniqueness of (organization_id, external_id) across all calls
DROP INDEX IF EXISTS idx_calls_external_id;
CREATE UNIQUE INDEX IF NOT EXISTS idx_calls_org_external_id ON calls(organization_id, external_id) WHERE external_id IS NOT NULL;
