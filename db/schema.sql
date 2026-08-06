-- Canonical quote persistence for PostgreSQL.
--
-- Apply through the reviewed Canonical migration identity. The runtime login
-- must be a non-owner, non-superuser, non-BYPASSRLS role with only the grants
-- listed at the end of this file.

BEGIN;

CREATE TABLE IF NOT EXISTS canonical_context (
    id uuid PRIMARY KEY,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    context_markdown text NOT NULL DEFAULT '',
    context_json jsonb NOT NULL DEFAULT '{}'::jsonb,
    active boolean NOT NULL DEFAULT TRUE,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (id, owner_subject)
);

CREATE TABLE IF NOT EXISTS canonical_quote (
    id uuid PRIMARY KEY,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    context_record_id uuid NOT NULL,
    request_json jsonb NOT NULL,
    application_context_markdown text NOT NULL,
    context_snapshot_markdown text NOT NULL,
    context_snapshot_json jsonb NOT NULL,
    gemini_model text NOT NULL CHECK (char_length(gemini_model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'completed', 'failed')),
    analysis_json jsonb,
    error_code text CHECK (error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_quote_context_owner_fk
        FOREIGN KEY (context_record_id, owner_subject)
        REFERENCES canonical_context (id, owner_subject)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS canonical_quote_event (
    sequence_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    quote_id uuid NOT NULL REFERENCES canonical_quote (id) ON DELETE CASCADE,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'completed', 'failed')),
    details_json jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS canonical_model_attempt (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL REFERENCES canonical_quote (id) ON DELETE CASCADE,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    provider text NOT NULL DEFAULT 'google-gemini' CHECK (provider = 'google-gemini'),
    model text NOT NULL CHECK (char_length(model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
    error_code text CHECK (error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120),
    started_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at timestamptz,
    CHECK (
        (status = 'started' AND finished_at IS NULL)
        OR (status IN ('completed', 'failed') AND finished_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS canonical_context_owner_active_idx
    ON canonical_context (owner_subject, active, updated_at DESC);
CREATE INDEX IF NOT EXISTS canonical_quote_owner_created_idx
    ON canonical_quote (owner_subject, created_at DESC);
CREATE INDEX IF NOT EXISTS canonical_quote_event_quote_sequence_idx
    ON canonical_quote_event (quote_id, sequence_id);
CREATE INDEX IF NOT EXISTS canonical_model_attempt_quote_started_idx
    ON canonical_model_attempt (quote_id, started_at DESC);

CREATE OR REPLACE FUNCTION canonical_set_updated_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS canonical_context_set_updated_at ON canonical_context;
CREATE TRIGGER canonical_context_set_updated_at
BEFORE UPDATE ON canonical_context
FOR EACH ROW EXECUTE FUNCTION canonical_set_updated_at();

DROP TRIGGER IF EXISTS canonical_quote_set_updated_at ON canonical_quote;
CREATE TRIGGER canonical_quote_set_updated_at
BEFORE UPDATE ON canonical_quote
FOR EACH ROW EXECUTE FUNCTION canonical_set_updated_at();

ALTER TABLE canonical_context ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_context FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_event ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_event FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_model_attempt ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_model_attempt FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS canonical_context_owner_policy ON canonical_context;
CREATE POLICY canonical_context_owner_policy ON canonical_context
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));

DROP POLICY IF EXISTS canonical_quote_owner_policy ON canonical_quote;
CREATE POLICY canonical_quote_owner_policy ON canonical_quote
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));

DROP POLICY IF EXISTS canonical_quote_event_owner_policy ON canonical_quote_event;
CREATE POLICY canonical_quote_event_owner_policy ON canonical_quote_event
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));

DROP POLICY IF EXISTS canonical_model_attempt_owner_policy ON canonical_model_attempt;
CREATE POLICY canonical_model_attempt_owner_policy ON canonical_model_attempt
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));

COMMENT ON TABLE canonical_context IS
    'Owner-scoped operational context selected for Canonical quote analysis.';
COMMENT ON TABLE canonical_quote IS
    'Durable quote request, immutable context snapshots, and bounded model result.';
COMMENT ON TABLE canonical_quote_event IS
    'Append-only status events; WebSocket broadcasts are disposable projections.';
COMMENT ON TABLE canonical_model_attempt IS
    'Provider attempt metadata only; raw prompts and API keys are never stored here.';

-- Deployment-owned grants (replace canonical_api_server only if the reviewed
-- runtime role has a different exact name):
--
-- GRANT SELECT, INSERT, UPDATE ON canonical_context TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_quote TO canonical_api_server;
-- GRANT SELECT, INSERT ON canonical_quote_event TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_model_attempt TO canonical_api_server;
-- GRANT USAGE, SELECT ON SEQUENCE canonical_quote_event_sequence_id_seq
--     TO canonical_api_server;
--
-- Do not grant DELETE, TRUNCATE, table ownership, schema ownership, SUPERUSER,
-- role membership, or BYPASSRLS to the runtime login.

COMMIT;
