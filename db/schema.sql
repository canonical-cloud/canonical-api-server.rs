CREATE SCHEMA canonical_cloud__quote;

CREATE TABLE canonical_cloud__quote.canonical_context (
    id uuid PRIMARY KEY,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    context_markdown text NOT NULL DEFAULT '',
    context_json jsonb NOT NULL DEFAULT '{}'::jsonb,
    active boolean NOT NULL DEFAULT TRUE,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_context_json_object_check
        CHECK (jsonb_typeof(context_json) = 'object'),
    CONSTRAINT canonical_context_id_owner_unique
        UNIQUE (id, owner_subject)
);

CREATE TABLE canonical_cloud__quote.canonical_quote (
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
    error_code text CHECK (
        error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120
    ),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_quote_request_json_object_check
        CHECK (jsonb_typeof(request_json) = 'object'),
    CONSTRAINT canonical_quote_context_snapshot_json_object_check
        CHECK (jsonb_typeof(context_snapshot_json) = 'object'),
    CONSTRAINT canonical_quote_analysis_json_object_check
        CHECK (
            analysis_json IS NULL
            OR jsonb_typeof(analysis_json) = 'object'
        ),
    CONSTRAINT canonical_quote_id_owner_unique
        UNIQUE (id, owner_subject),
    CONSTRAINT canonical_quote_context_owner_fk
        FOREIGN KEY (context_record_id, owner_subject)
        REFERENCES canonical_cloud__quote.canonical_context (id, owner_subject)
        ON DELETE RESTRICT
);

CREATE TABLE canonical_cloud__quote.canonical_quote_operation (
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    idempotency_key text NOT NULL,
    operation text NOT NULL CHECK (operation IN ('create', 'retry')),
    quote_id uuid NOT NULL,
    request_json jsonb,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_quote_operation_owner_key_pk
        PRIMARY KEY (owner_subject, idempotency_key),
    CONSTRAINT canonical_quote_operation_key_check
        CHECK (
            char_length(idempotency_key) BETWEEN 8 AND 128
            AND idempotency_key ~ '^[A-Za-z0-9._:-]+$'
        ),
    CONSTRAINT canonical_quote_operation_request_check
        CHECK (
            (operation = 'create' AND request_json IS NOT NULL)
            OR (operation = 'retry' AND request_json IS NULL)
        ),
    CONSTRAINT canonical_quote_operation_request_json_object_check
        CHECK (
            request_json IS NULL
            OR jsonb_typeof(request_json) = 'object'
        ),
    CONSTRAINT canonical_quote_operation_quote_owner_fk
        FOREIGN KEY (quote_id, owner_subject)
        REFERENCES canonical_cloud__quote.canonical_quote (id, owner_subject)
        ON DELETE CASCADE
);

CREATE TABLE canonical_cloud__quote.canonical_quote_event (
    sequence_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    quote_id uuid NOT NULL,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'completed', 'failed')),
    details_json jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_quote_event_details_json_object_check
        CHECK (jsonb_typeof(details_json) = 'object'),
    CONSTRAINT canonical_quote_event_quote_owner_fk
        FOREIGN KEY (quote_id, owner_subject)
        REFERENCES canonical_cloud__quote.canonical_quote (id, owner_subject)
        ON DELETE CASCADE
);

CREATE TABLE canonical_cloud__quote.canonical_model_attempt (
    id uuid PRIMARY KEY,
    quote_id uuid NOT NULL,
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    provider text NOT NULL DEFAULT 'google-gemini'
        CHECK (provider = 'google-gemini'),
    model text NOT NULL CHECK (char_length(model) BETWEEN 1 AND 128),
    status text NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
    error_code text CHECK (
        error_code IS NULL OR char_length(error_code) BETWEEN 1 AND 120
    ),
    started_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at timestamptz,
    CONSTRAINT canonical_model_attempt_status_finished_check CHECK (
        (status = 'started' AND finished_at IS NULL)
        OR (status IN ('completed', 'failed') AND finished_at IS NOT NULL)
    ),
    CONSTRAINT canonical_model_attempt_time_order_check CHECK (
        finished_at IS NULL OR finished_at >= started_at
    ),
    CONSTRAINT canonical_model_attempt_quote_owner_fk
        FOREIGN KEY (quote_id, owner_subject)
        REFERENCES canonical_cloud__quote.canonical_quote (id, owner_subject)
        ON DELETE CASCADE
);

CREATE TABLE canonical_cloud__quote.canonical_readiness_observation (
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    source_id text NOT NULL,
    event_id text NOT NULL,
    source_sequence bigint NOT NULL,
    organization text NOT NULL CHECK (char_length(organization) BETWEEN 1 AND 200),
    observed_at timestamptz NOT NULL,
    payload_sha256 text NOT NULL,
    prior_record_sha256 text NOT NULL,
    record_sha256 text NOT NULL,
    receipt_id text NOT NULL,
    key_id text NOT NULL,
    event_json jsonb NOT NULL,
    raw_body_octets bigint NOT NULL,
    transport_verification text NOT NULL,
    substantive_review text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_readiness_observation_owner_source_event_pk
        PRIMARY KEY (owner_subject, source_id, event_id),
    CONSTRAINT canonical_readiness_observation_owner_source_sequence_unique
        UNIQUE (owner_subject, source_id, source_sequence),
    CONSTRAINT canonical_readiness_observation_receipt_id_unique
        UNIQUE (receipt_id),
    CONSTRAINT canonical_readiness_observation_source_id_check CHECK (
        char_length(source_id) BETWEEN 1 AND 128
        AND source_id ~ '^[A-Za-z0-9._:/-]+$'
    ),
    CONSTRAINT canonical_readiness_observation_event_id_check CHECK (
        char_length(event_id) BETWEEN 1 AND 128
        AND event_id ~ '^[A-Za-z0-9._:/-]+$'
    ),
    CONSTRAINT canonical_readiness_observation_source_sequence_check
        CHECK (source_sequence > 0),
    CONSTRAINT canonical_readiness_observation_payload_sha256_check
        CHECK (payload_sha256 ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT canonical_readiness_observation_prior_record_sha256_check
        CHECK (prior_record_sha256 ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT canonical_readiness_observation_record_sha256_check
        CHECK (record_sha256 ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT canonical_readiness_observation_receipt_id_check CHECK (
        char_length(receipt_id) BETWEEN 8 AND 128
        AND receipt_id ~ '^[A-Za-z0-9._:/-]+$'
    ),
    CONSTRAINT canonical_readiness_observation_key_id_check CHECK (
        char_length(key_id) BETWEEN 1 AND 128
        AND key_id ~ '^[A-Za-z0-9._:/-]+$'
    ),
    CONSTRAINT canonical_readiness_observation_event_json_object_check
        CHECK (jsonb_typeof(event_json) = 'object'),
    CONSTRAINT canonical_readiness_observation_raw_body_octets_check
        CHECK (raw_body_octets BETWEEN 1 AND 262144),
    CONSTRAINT canonical_readiness_observation_transport_verification_check
        CHECK (transport_verification = 'signature-valid'),
    CONSTRAINT canonical_readiness_observation_substantive_review_check CHECK (
        substantive_review IN (
            'unreviewed',
            'in-review',
            'accepted-as-evidence',
            'rejected',
            'superseded'
        )
    )
);

CREATE INDEX canonical_context_owner_active_idx
    ON canonical_cloud__quote.canonical_context (
        owner_subject,
        active,
        updated_at DESC
    );

CREATE UNIQUE INDEX canonical_context_one_active_per_owner_idx
    ON canonical_cloud__quote.canonical_context (owner_subject)
    WHERE active = TRUE;

CREATE INDEX canonical_quote_owner_created_idx
    ON canonical_cloud__quote.canonical_quote (
        owner_subject,
        created_at DESC,
        id DESC
    );

CREATE INDEX canonical_quote_operation_quote_created_idx
    ON canonical_cloud__quote.canonical_quote_operation (
        quote_id,
        created_at DESC
    );

CREATE INDEX canonical_quote_event_quote_sequence_idx
    ON canonical_cloud__quote.canonical_quote_event (
        quote_id,
        sequence_id
    );

CREATE INDEX canonical_model_attempt_quote_started_idx
    ON canonical_cloud__quote.canonical_model_attempt (
        quote_id,
        started_at DESC
    );

CREATE INDEX canonical_readiness_observation_owner_received_idx
    ON canonical_cloud__quote.canonical_readiness_observation (
        owner_subject,
        received_at DESC,
        source_id,
        source_sequence DESC
    );

CREATE FUNCTION canonical_cloud__quote.canonical_set_updated_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$;

CREATE TRIGGER canonical_context_set_updated_at
BEFORE UPDATE ON canonical_cloud__quote.canonical_context
FOR EACH ROW
EXECUTE FUNCTION canonical_cloud__quote.canonical_set_updated_at();

CREATE TRIGGER canonical_quote_set_updated_at
BEFORE UPDATE ON canonical_cloud__quote.canonical_quote
FOR EACH ROW
EXECUTE FUNCTION canonical_cloud__quote.canonical_set_updated_at();

ALTER TABLE canonical_cloud__quote.canonical_context
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_context
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote_operation
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote_operation
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote_event
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_quote_event
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_model_attempt
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_model_attempt
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_readiness_observation
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_readiness_observation
    FORCE ROW LEVEL SECURITY;

CREATE POLICY canonical_context_owner_policy
ON canonical_cloud__quote.canonical_context
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

CREATE POLICY canonical_quote_owner_policy
ON canonical_cloud__quote.canonical_quote
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

CREATE POLICY canonical_quote_operation_owner_policy
ON canonical_cloud__quote.canonical_quote_operation
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

CREATE POLICY canonical_quote_event_owner_policy
ON canonical_cloud__quote.canonical_quote_event
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

CREATE POLICY canonical_model_attempt_owner_policy
ON canonical_cloud__quote.canonical_model_attempt
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

CREATE POLICY canonical_readiness_observation_owner_policy
ON canonical_cloud__quote.canonical_readiness_observation
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);
