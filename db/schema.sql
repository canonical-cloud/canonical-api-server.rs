-- Canonical quote persistence for PostgreSQL.
--
-- Apply through the reviewed Canonical migration identity. The HTTP and worker
-- logins must be distinct, non-owner, non-superuser, non-BYPASSRLS roles with
-- only the grants listed at the end of this file.

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

CREATE TABLE IF NOT EXISTS canonical_quote_contact_selection (
    id uuid PRIMARY KEY,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    email_ciphertext text NOT NULL CHECK (char_length(email_ciphertext) BETWEEN 32 AND 2048),
    phone_ciphertext text NOT NULL CHECK (char_length(phone_ciphertext) BETWEEN 32 AND 512),
    email_masked text NOT NULL CHECK (char_length(email_masked) BETWEEN 3 AND 320),
    phone_masked text NOT NULL CHECK (char_length(phone_masked) BETWEEN 4 AND 32),
    email_confirmed_at timestamptz NOT NULL,
    phone_confirmed_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_quote_id uuid,
    consumed_revision integer CHECK (consumed_revision IS NULL OR consumed_revision >= 1),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (id, owner_subject),
    CHECK (
        (consumed_quote_id IS NULL AND consumed_revision IS NULL)
        OR (consumed_quote_id IS NOT NULL AND consumed_revision IS NOT NULL)
    )
);

CREATE TABLE IF NOT EXISTS canonical_quote (
    id uuid PRIMARY KEY,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    contact_selection_id uuid NOT NULL,
    current_revision integer NOT NULL DEFAULT 1 CHECK (current_revision >= 1),
    context_record_id uuid NOT NULL,
    request_json jsonb NOT NULL,
    application_context_markdown text NOT NULL,
    context_snapshot_markdown text NOT NULL,
    context_snapshot_json jsonb NOT NULL,
    gemini_model text NOT NULL CHECK (char_length(gemini_model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
    analysis_json jsonb,
    error_code text CHECK (error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120),
    access_link_expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (id, owner_subject),
    CONSTRAINT canonical_quote_context_owner_fk
        FOREIGN KEY (context_record_id, owner_subject)
        REFERENCES canonical_context (id, owner_subject)
        ON DELETE RESTRICT,
    CONSTRAINT canonical_quote_contact_owner_fk
        FOREIGN KEY (contact_selection_id, owner_subject)
        REFERENCES canonical_quote_contact_selection (id, owner_subject)
        ON DELETE RESTRICT
);

-- Upgrade columns for installations created by the original v1 schema. The
-- migration runner must backfill contact selections before enforcing the new
-- NOT NULL contact/link columns in an already-populated production database.
ALTER TABLE canonical_quote
    ADD COLUMN IF NOT EXISTS contact_selection_id uuid,
    ADD COLUMN IF NOT EXISTS current_revision integer NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS access_link_expires_at timestamptz;
UPDATE canonical_quote SET status = 'ready' WHERE status = 'completed';
ALTER TABLE canonical_quote DROP CONSTRAINT IF EXISTS canonical_quote_status_check;
ALTER TABLE canonical_quote ADD CONSTRAINT canonical_quote_status_check
    CHECK (status IN ('queued', 'analyzing', 'ready', 'failed'));

CREATE TABLE IF NOT EXISTS canonical_quote_revision (
    quote_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    contact_selection_id uuid NOT NULL,
    context_record_id uuid NOT NULL,
    request_json jsonb NOT NULL,
    application_context_markdown text NOT NULL,
    context_snapshot_markdown text NOT NULL,
    context_snapshot_json jsonb NOT NULL,
    gemini_model text NOT NULL CHECK (char_length(gemini_model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
    analysis_json jsonb,
    error_code text CHECK (error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (quote_id, revision),
    CONSTRAINT canonical_quote_revision_quote_owner_fk
        FOREIGN KEY (quote_id, owner_subject)
        REFERENCES canonical_quote (id, owner_subject)
        ON DELETE CASCADE,
    CONSTRAINT canonical_quote_revision_context_owner_fk
        FOREIGN KEY (context_record_id, owner_subject)
        REFERENCES canonical_context (id, owner_subject)
        ON DELETE RESTRICT,
    CONSTRAINT canonical_quote_revision_contact_owner_fk
        FOREIGN KEY (contact_selection_id, owner_subject)
        REFERENCES canonical_quote_contact_selection (id, owner_subject)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS canonical_quote_event (
    sequence_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    quote_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
    details_json jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (quote_id, revision)
        REFERENCES canonical_quote_revision (quote_id, revision)
        ON DELETE CASCADE
);
ALTER TABLE canonical_quote_event
    ADD COLUMN IF NOT EXISTS revision integer NOT NULL DEFAULT 1;
ALTER TABLE canonical_quote_event DROP CONSTRAINT IF EXISTS canonical_quote_event_status_check;
ALTER TABLE canonical_quote_event ADD CONSTRAINT canonical_quote_event_status_check
    CHECK (status IN ('queued', 'analyzing', 'ready', 'failed'));

CREATE TABLE IF NOT EXISTS canonical_quote_job (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 20),
    next_attempt_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    lease_owner uuid,
    lease_expires_at timestamptz,
    last_error_code text CHECK (last_error_code IS NULL OR char_length(last_error_code) BETWEEN 1 AND 120),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (quote_id, revision),
    FOREIGN KEY (quote_id, revision)
        REFERENCES canonical_quote_revision (quote_id, revision)
        ON DELETE CASCADE,
    CHECK (
        (status = 'running' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (status <> 'running')
    )
);

CREATE TABLE IF NOT EXISTS canonical_model_attempt (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    provider text NOT NULL DEFAULT 'google-gemini' CHECK (provider = 'google-gemini'),
    model text NOT NULL CHECK (char_length(model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
    error_code text CHECK (error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120),
    started_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at timestamptz,
    FOREIGN KEY (quote_id, revision)
        REFERENCES canonical_quote_revision (quote_id, revision)
        ON DELETE CASCADE,
    CHECK (
        (status = 'started' AND finished_at IS NULL)
        OR (status IN ('completed', 'failed') AND finished_at IS NOT NULL)
    )
);
ALTER TABLE canonical_model_attempt
    ADD COLUMN IF NOT EXISTS revision integer NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS canonical_quote_access_link (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    token_digest text NOT NULL UNIQUE CHECK (char_length(token_digest) = 43),
    token_ciphertext text NOT NULL CHECK (char_length(token_ciphertext) BETWEEN 32 AND 2048),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (quote_id, owner_subject)
        REFERENCES canonical_quote (id, owner_subject)
        ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS canonical_quote_one_active_link_idx
    ON canonical_quote_access_link (quote_id)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS canonical_notification_outbox (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    access_link_id uuid NOT NULL REFERENCES canonical_quote_access_link (id) ON DELETE RESTRICT,
    contact_selection_id uuid NOT NULL,
    kind text NOT NULL CHECK (kind IN ('sms_quote_link')),
    deduplication_key text NOT NULL UNIQUE CHECK (char_length(deduplication_key) BETWEEN 8 AND 255),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sending', 'sent', 'failed')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 20),
    next_attempt_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    lease_owner uuid,
    lease_expires_at timestamptz,
    provider_message_id text CHECK (provider_message_id IS NULL OR char_length(provider_message_id) BETWEEN 1 AND 255),
    last_error_code text CHECK (last_error_code IS NULL OR char_length(last_error_code) BETWEEN 1 AND 120),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (quote_id, revision)
        REFERENCES canonical_quote_revision (quote_id, revision)
        ON DELETE CASCADE,
    FOREIGN KEY (contact_selection_id, owner_subject)
        REFERENCES canonical_quote_contact_selection (id, owner_subject)
        ON DELETE RESTRICT,
    CHECK (
        (status = 'sending' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (status <> 'sending')
    )
);

CREATE INDEX IF NOT EXISTS canonical_context_owner_active_idx
    ON canonical_context (owner_subject, active, updated_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS canonical_context_one_active_per_owner_idx
    ON canonical_context (owner_subject) WHERE active = TRUE;
CREATE INDEX IF NOT EXISTS canonical_quote_owner_created_idx
    ON canonical_quote (owner_subject, created_at DESC);
CREATE INDEX IF NOT EXISTS canonical_quote_revision_owner_created_idx
    ON canonical_quote_revision (owner_subject, created_at DESC);
CREATE INDEX IF NOT EXISTS canonical_quote_event_quote_sequence_idx
    ON canonical_quote_event (quote_id, sequence_id);
CREATE INDEX IF NOT EXISTS canonical_quote_job_claim_idx
    ON canonical_quote_job (next_attempt_at, created_at)
    WHERE status IN ('queued', 'running');
CREATE INDEX IF NOT EXISTS canonical_model_attempt_quote_started_idx
    ON canonical_model_attempt (quote_id, revision, started_at DESC);
CREATE INDEX IF NOT EXISTS canonical_notification_claim_idx
    ON canonical_notification_outbox (next_attempt_at, created_at)
    WHERE status IN ('pending', 'sending');

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
DROP TRIGGER IF EXISTS canonical_quote_revision_set_updated_at ON canonical_quote_revision;
CREATE TRIGGER canonical_quote_revision_set_updated_at
BEFORE UPDATE ON canonical_quote_revision
FOR EACH ROW EXECUTE FUNCTION canonical_set_updated_at();
DROP TRIGGER IF EXISTS canonical_quote_job_set_updated_at ON canonical_quote_job;
CREATE TRIGGER canonical_quote_job_set_updated_at
BEFORE UPDATE ON canonical_quote_job
FOR EACH ROW EXECUTE FUNCTION canonical_set_updated_at();
DROP TRIGGER IF EXISTS canonical_notification_set_updated_at ON canonical_notification_outbox;
CREATE TRIGGER canonical_notification_set_updated_at
BEFORE UPDATE ON canonical_notification_outbox
FOR EACH ROW EXECUTE FUNCTION canonical_set_updated_at();

ALTER TABLE canonical_context ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_context FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_contact_selection ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_contact_selection FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_revision ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_revision FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_event ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_event FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_job ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_job FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_model_attempt ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_model_attempt FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_access_link ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_quote_access_link FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_notification_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_notification_outbox FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS canonical_context_owner_policy ON canonical_context;
CREATE POLICY canonical_context_owner_policy ON canonical_context
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_context_worker_policy ON canonical_context;
CREATE POLICY canonical_context_worker_policy ON canonical_context
    TO canonical_quote_worker USING (TRUE) WITH CHECK (FALSE);

DROP POLICY IF EXISTS canonical_quote_contact_owner_policy ON canonical_quote_contact_selection;
CREATE POLICY canonical_quote_contact_owner_policy ON canonical_quote_contact_selection
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_contact_worker_policy ON canonical_quote_contact_selection;
CREATE POLICY canonical_quote_contact_worker_policy ON canonical_quote_contact_selection
    TO canonical_quote_worker USING (TRUE) WITH CHECK (FALSE);

DROP POLICY IF EXISTS canonical_quote_owner_policy ON canonical_quote;
CREATE POLICY canonical_quote_owner_policy ON canonical_quote
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_worker_policy ON canonical_quote;
CREATE POLICY canonical_quote_worker_policy ON canonical_quote
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

DROP POLICY IF EXISTS canonical_quote_revision_owner_policy ON canonical_quote_revision;
CREATE POLICY canonical_quote_revision_owner_policy ON canonical_quote_revision
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_revision_worker_policy ON canonical_quote_revision;
CREATE POLICY canonical_quote_revision_worker_policy ON canonical_quote_revision
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

DROP POLICY IF EXISTS canonical_quote_event_owner_policy ON canonical_quote_event;
CREATE POLICY canonical_quote_event_owner_policy ON canonical_quote_event
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_event_worker_policy ON canonical_quote_event;
CREATE POLICY canonical_quote_event_worker_policy ON canonical_quote_event
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

DROP POLICY IF EXISTS canonical_quote_job_owner_policy ON canonical_quote_job;
CREATE POLICY canonical_quote_job_owner_policy ON canonical_quote_job
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_job_worker_policy ON canonical_quote_job;
CREATE POLICY canonical_quote_job_worker_policy ON canonical_quote_job
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

DROP POLICY IF EXISTS canonical_model_attempt_owner_policy ON canonical_model_attempt;
CREATE POLICY canonical_model_attempt_owner_policy ON canonical_model_attempt
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_model_attempt_worker_policy ON canonical_model_attempt;
CREATE POLICY canonical_model_attempt_worker_policy ON canonical_model_attempt
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

DROP POLICY IF EXISTS canonical_quote_access_owner_policy ON canonical_quote_access_link;
CREATE POLICY canonical_quote_access_owner_policy ON canonical_quote_access_link
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_quote_access_worker_policy ON canonical_quote_access_link;
CREATE POLICY canonical_quote_access_worker_policy ON canonical_quote_access_link
    TO canonical_quote_worker USING (TRUE) WITH CHECK (FALSE);

DROP POLICY IF EXISTS canonical_notification_owner_policy ON canonical_notification_outbox;
CREATE POLICY canonical_notification_owner_policy ON canonical_notification_outbox
    USING (owner_subject = current_setting('app.current_subject', TRUE))
    WITH CHECK (owner_subject = current_setting('app.current_subject', TRUE));
DROP POLICY IF EXISTS canonical_notification_worker_policy ON canonical_notification_outbox;
CREATE POLICY canonical_notification_worker_policy ON canonical_notification_outbox
    TO canonical_quote_worker USING (TRUE) WITH CHECK (TRUE);

CREATE OR REPLACE FUNCTION canonical_redeem_quote_link(p_token_digest text)
RETURNS TABLE (quote_id uuid, owner_subject text, expires_at timestamptz)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NOT pg_has_role(session_user, 'canonical_api_server', 'member') THEN
        RAISE EXCEPTION 'canonical_redeem_quote_link caller is not authorized';
    END IF;
    RETURN QUERY
    UPDATE public.canonical_quote_access_link AS link
       SET last_used_at = CURRENT_TIMESTAMP
     WHERE link.token_digest = p_token_digest
       AND link.revoked_at IS NULL
       AND link.expires_at > CURRENT_TIMESTAMP
    RETURNING link.quote_id, link.owner_subject, link.expires_at;
END;
$$;
REVOKE ALL ON FUNCTION canonical_redeem_quote_link(text) FROM PUBLIC;

COMMENT ON TABLE canonical_quote_contact_selection IS
    'Encrypted verified contacts plus explicit Canonical quote-contact consent; never identity authority.';
COMMENT ON TABLE canonical_quote IS
    'Stable owner-scoped quote header and current revision projection.';
COMMENT ON TABLE canonical_quote_revision IS
    'Immutable submitted inputs/context and the bounded result for each quote revision.';
COMMENT ON TABLE canonical_quote_job IS
    'Durable leased analysis jobs recoverable after process failure.';
COMMENT ON TABLE canonical_quote_event IS
    'Append-only revision status events; WebSocket broadcasts are disposable projections.';
COMMENT ON TABLE canonical_quote_access_link IS
    'Revocable 25-day quote capabilities; plaintext tokens are encrypted and digests support lookup.';
COMMENT ON TABLE canonical_notification_outbox IS
    'Deduplicated durable product-message work; Twilio Verify is not used for these messages.';
COMMENT ON TABLE canonical_model_attempt IS
    'Provider attempt metadata only; raw prompts, responses, contacts, and API keys are never logged.';

-- Deployment-owned grants (create the exact NOLOGIN group roles first and
-- grant login roles membership only in their one matching group):
--
-- HTTP role canonical_api_server:
-- GRANT SELECT, INSERT, UPDATE ON canonical_context TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_quote_contact_selection TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_quote TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_quote_revision TO canonical_api_server;
-- GRANT SELECT, INSERT ON canonical_quote_event TO canonical_api_server;
-- GRANT SELECT, INSERT, UPDATE ON canonical_quote_job TO canonical_api_server;
-- GRANT SELECT, INSERT ON canonical_quote_access_link TO canonical_api_server;
-- GRANT SELECT, INSERT ON canonical_notification_outbox TO canonical_api_server;
-- GRANT EXECUTE ON FUNCTION canonical_redeem_quote_link(text) TO canonical_api_server;
--
-- Worker role canonical_quote_worker:
-- GRANT SELECT ON canonical_context, canonical_quote_contact_selection,
--     canonical_quote_access_link TO canonical_quote_worker;
-- GRANT SELECT, UPDATE ON canonical_quote, canonical_quote_revision,
--     canonical_quote_job, canonical_notification_outbox TO canonical_quote_worker;
-- GRANT SELECT, INSERT ON canonical_quote_event TO canonical_quote_worker;
-- GRANT SELECT, INSERT, UPDATE ON canonical_model_attempt TO canonical_quote_worker;
--
-- Both roles:
-- GRANT USAGE, SELECT ON SEQUENCE canonical_quote_event_sequence_id_seq
--     TO canonical_api_server, canonical_quote_worker;
--
-- Do not grant DELETE, TRUNCATE, table/schema ownership, SUPERUSER, role
-- administration, or BYPASSRLS to either runtime login.

COMMIT;
