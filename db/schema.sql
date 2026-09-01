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

CREATE TABLE canonical_cloud__quote.canonical_pre_interest_registration (
    id uuid PRIMARY KEY,
    normalized_email text NOT NULL,
    email_alias text NOT NULL,
    party_type text NOT NULL,
    organization_name text,
    first_source_host text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_pre_interest_registration_email_check CHECK (
        char_length(normalized_email) BETWEEN 3 AND 254
        AND normalized_email = lower(normalized_email)
        AND position('@' IN normalized_email) > 1
    ),
    CONSTRAINT canonical_pre_interest_registration_alias_check CHECK (
        email_alias ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT canonical_pre_interest_registration_party_type_check CHECK (
        party_type IN ('individual', 'organization')
    ),
    CONSTRAINT canonical_pre_interest_registration_org_name_check CHECK (
        organization_name IS NULL
        OR char_length(organization_name) BETWEEN 1 AND 200
    ),
    CONSTRAINT canonical_pre_interest_registration_source_host_check CHECK (
        first_source_host IN ('user.canonical.plus', 'org.canonical.plus')
    ),
    CONSTRAINT canonical_pre_interest_registration_party_org_check CHECK (
        (party_type = 'individual' AND organization_name IS NULL)
        OR (party_type = 'organization' AND organization_name IS NOT NULL)
    ),
    CONSTRAINT canonical_pre_interest_registration_alias_party_unique
        UNIQUE (email_alias, party_type)
);

CREATE TABLE canonical_cloud__quote.canonical_pre_interest_consent (
    id uuid PRIMARY KEY,
    registration_id uuid NOT NULL,
    request_alias text NOT NULL,
    consent_version text NOT NULL,
    interest_areas jsonb NOT NULL,
    organization_name text,
    source_host text NOT NULL,
    locale text,
    referral_code text,
    consented_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT canonical_pre_interest_consent_request_alias_unique
        UNIQUE (request_alias),
    CONSTRAINT canonical_pre_interest_consent_request_alias_check CHECK (
        request_alias ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT canonical_pre_interest_consent_version_check CHECK (
        char_length(consent_version) BETWEEN 1 AND 64
        AND consent_version ~ '^[A-Za-z0-9._:-]+$'
    ),
    CONSTRAINT canonical_pre_interest_consent_interests_array_check CHECK (
        jsonb_typeof(interest_areas) = 'array'
        AND jsonb_array_length(interest_areas) BETWEEN 1 AND 20
    ),
    CONSTRAINT canonical_pre_interest_consent_org_name_check CHECK (
        organization_name IS NULL
        OR char_length(organization_name) BETWEEN 1 AND 200
    ),
    CONSTRAINT canonical_pre_interest_consent_source_host_check CHECK (
        source_host IN ('user.canonical.plus', 'org.canonical.plus')
    ),
    CONSTRAINT canonical_pre_interest_consent_locale_check CHECK (
        locale IS NULL
        OR (
            char_length(locale) BETWEEN 2 AND 35
            AND locale ~ '^[A-Za-z0-9-]+$'
        )
    ),
    CONSTRAINT canonical_pre_interest_consent_referral_check CHECK (
        referral_code IS NULL
        OR (
            char_length(referral_code) BETWEEN 1 AND 64
            AND referral_code ~ '^[A-Za-z0-9._:-]+$'
        )
    ),
    CONSTRAINT canonical_pre_interest_consent_registration_fk
        FOREIGN KEY (registration_id)
        REFERENCES canonical_cloud__quote.canonical_pre_interest_registration (id)
        ON DELETE RESTRICT
);

CREATE TABLE canonical_cloud__quote.canonical_pre_interest_outbox (
    id uuid PRIMARY KEY,
    registration_id uuid NOT NULL,
    consent_id uuid NOT NULL,
    event_type text NOT NULL,
    payload_json jsonb NOT NULL,
    status text NOT NULL DEFAULT 'pending',
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    lease_owner text,
    lease_expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    sent_at timestamptz,
    CONSTRAINT canonical_pre_interest_outbox_consent_unique
        UNIQUE (consent_id),
    CONSTRAINT canonical_pre_interest_outbox_event_type_check CHECK (
        event_type = 'pre_interest.accepted.v1'
    ),
    CONSTRAINT canonical_pre_interest_outbox_status_check CHECK (
        status IN ('pending', 'delivering', 'sent', 'dead')
    ),
    CONSTRAINT canonical_pre_interest_outbox_attempts_check CHECK (
        attempts BETWEEN 0 AND 100
    ),
    CONSTRAINT canonical_pre_interest_outbox_payload_object_check CHECK (
        jsonb_typeof(payload_json) = 'object'
    ),
    CONSTRAINT canonical_pre_interest_outbox_lease_check CHECK (
        (status = 'delivering' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (status <> 'delivering' AND lease_owner IS NULL AND lease_expires_at IS NULL)
    ),
    CONSTRAINT canonical_pre_interest_outbox_sent_check CHECK (
        (status = 'sent' AND sent_at IS NOT NULL)
        OR (status <> 'sent' AND sent_at IS NULL)
    ),
    CONSTRAINT canonical_pre_interest_outbox_registration_fk
        FOREIGN KEY (registration_id)
        REFERENCES canonical_cloud__quote.canonical_pre_interest_registration (id)
        ON DELETE RESTRICT,
    CONSTRAINT canonical_pre_interest_outbox_consent_fk
        FOREIGN KEY (consent_id)
        REFERENCES canonical_cloud__quote.canonical_pre_interest_consent (id)
        ON DELETE RESTRICT
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

CREATE INDEX canonical_pre_interest_registration_created_idx
    ON canonical_cloud__quote.canonical_pre_interest_registration (
        created_at DESC,
        id DESC
    );

CREATE INDEX canonical_pre_interest_consent_registration_time_idx
    ON canonical_cloud__quote.canonical_pre_interest_consent (
        registration_id,
        consented_at DESC,
        id DESC
    );

CREATE INDEX canonical_pre_interest_outbox_delivery_idx
    ON canonical_cloud__quote.canonical_pre_interest_outbox (
        status,
        next_attempt_at,
        created_at,
        id
    )
    WHERE status IN ('pending', 'delivering');

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
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_registration
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_registration
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_consent
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_consent
    FORCE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_outbox
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__quote.canonical_pre_interest_outbox
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

CREATE POLICY canonical_pre_interest_registration_write_policy
ON canonical_cloud__quote.canonical_pre_interest_registration
USING (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
)
WITH CHECK (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
);

CREATE POLICY canonical_pre_interest_consent_write_policy
ON canonical_cloud__quote.canonical_pre_interest_consent
USING (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
)
WITH CHECK (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
);

CREATE POLICY canonical_pre_interest_outbox_write_policy
ON canonical_cloud__quote.canonical_pre_interest_outbox
USING (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
)
WITH CHECK (
    current_setting('canonical.pre_interest_write', TRUE) = '1'
);
