-- DEN-4058 candidate declarative schema.
--
-- This file is not part of db/namespace.json and is not authorized for
-- production application. Promotion requires an independently reviewed
-- namespace manifest, least-privilege grants, disposable PostgreSQL 17
-- execution, and Diesel/SeaORM operation-parity evidence.

CREATE SCHEMA canonical_cloud__interest;

CREATE TABLE canonical_cloud__interest.canonical_pre_interest_registration (
    registration_id uuid PRIMARY KEY,
    email_alias bytea NOT NULL,
    email_ciphertext bytea NOT NULL,
    party_type text NOT NULL,
    organization_name_ciphertext bytea,
    display_name_ciphertext bytea,
    website_url_ciphertext bytea,
    first_accepted_at timestamptz NOT NULL,
    last_accepted_at timestamptz NOT NULL,
    CONSTRAINT canonical_pre_interest_email_alias_length_check
        CHECK (octet_length(email_alias) = 32),
    CONSTRAINT canonical_pre_interest_email_alias_unique
        UNIQUE (email_alias),
    CONSTRAINT canonical_pre_interest_party_type_check
        CHECK (party_type IN ('individual', 'organization')),
    CONSTRAINT canonical_pre_interest_organization_presence_check
        CHECK (
            (party_type = 'individual' AND organization_name_ciphertext IS NULL)
            OR
            (party_type = 'organization' AND organization_name_ciphertext IS NOT NULL)
        ),
    CONSTRAINT canonical_pre_interest_acceptance_order_check
        CHECK (last_accepted_at >= first_accepted_at)
);

ALTER TABLE canonical_cloud__interest.canonical_pre_interest_registration
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__interest.canonical_pre_interest_registration
    FORCE ROW LEVEL SECURITY;

CREATE TABLE canonical_cloud__interest.canonical_pre_interest_submission (
    submission_id uuid PRIMARY KEY,
    registration_id uuid NOT NULL,
    request_id uuid NOT NULL,
    request_fingerprint bytea NOT NULL,
    receipt_id uuid NOT NULL,
    party_type text NOT NULL,
    source_host text NOT NULL,
    consent_revision text NOT NULL,
    registration_consent boolean NOT NULL,
    marketing_consent boolean NOT NULL,
    marketing_consent_revision text,
    client_consented_at timestamptz NOT NULL,
    accepted_at timestamptz NOT NULL,
    interest_areas text[] NOT NULL,
    locale text,
    referral_code_ciphertext bytea,
    CONSTRAINT canonical_pre_interest_submission_registration_fk
        FOREIGN KEY (registration_id)
        REFERENCES canonical_cloud__interest.canonical_pre_interest_registration (
            registration_id
        ),
    CONSTRAINT canonical_pre_interest_request_fingerprint_length_check
        CHECK (octet_length(request_fingerprint) = 32),
    CONSTRAINT canonical_pre_interest_request_id_unique
        UNIQUE (request_id),
    CONSTRAINT canonical_pre_interest_receipt_id_unique
        UNIQUE (receipt_id),
    CONSTRAINT canonical_pre_interest_submission_party_type_check
        CHECK (party_type IN ('individual', 'organization')),
    CONSTRAINT canonical_pre_interest_source_host_check
        CHECK (source_host IN ('user.canonical.plus', 'org.canonical.plus')),
    CONSTRAINT canonical_pre_interest_host_party_consistency_check
        CHECK (
            (source_host = 'user.canonical.plus' AND party_type = 'individual')
            OR
            (source_host = 'org.canonical.plus' AND party_type = 'organization')
        ),
    CONSTRAINT canonical_pre_interest_interest_count_check
        CHECK (cardinality(interest_areas) BETWEEN 1 AND 9),
    CONSTRAINT canonical_pre_interest_consent_revision_length_check
        CHECK (length(consent_revision) BETWEEN 1 AND 64),
    CONSTRAINT canonical_pre_interest_registration_consent_check
        CHECK (registration_consent),
    CONSTRAINT canonical_pre_interest_marketing_consent_consistency_check
        CHECK (
            (
                marketing_consent
                AND marketing_consent_revision IS NOT NULL
                AND length(marketing_consent_revision) BETWEEN 1 AND 64
            )
            OR
            (
                NOT marketing_consent
                AND marketing_consent_revision IS NULL
            )
        ),
    CONSTRAINT canonical_pre_interest_locale_length_check
        CHECK (locale IS NULL OR length(locale) BETWEEN 2 AND 35)
);

ALTER TABLE canonical_cloud__interest.canonical_pre_interest_submission
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__interest.canonical_pre_interest_submission
    FORCE ROW LEVEL SECURITY;

CREATE TABLE canonical_cloud__interest.canonical_pre_interest_consent_event (
    consent_event_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    submission_id uuid NOT NULL,
    registration_id uuid NOT NULL,
    consent_revision text NOT NULL,
    registration_consent boolean NOT NULL,
    marketing_consent boolean NOT NULL,
    marketing_consent_revision text,
    source_host text NOT NULL,
    client_consented_at timestamptz NOT NULL,
    accepted_at timestamptz NOT NULL,
    event_digest bytea NOT NULL,
    CONSTRAINT canonical_pre_interest_consent_submission_fk
        FOREIGN KEY (submission_id)
        REFERENCES canonical_cloud__interest.canonical_pre_interest_submission (
            submission_id
        ),
    CONSTRAINT canonical_pre_interest_consent_registration_fk
        FOREIGN KEY (registration_id)
        REFERENCES canonical_cloud__interest.canonical_pre_interest_registration (
            registration_id
        ),
    CONSTRAINT canonical_pre_interest_consent_registration_check
        CHECK (registration_consent),
    CONSTRAINT canonical_pre_interest_consent_marketing_consistency_check
        CHECK (
            (
                marketing_consent
                AND marketing_consent_revision IS NOT NULL
                AND length(marketing_consent_revision) BETWEEN 1 AND 64
            )
            OR
            (
                NOT marketing_consent
                AND marketing_consent_revision IS NULL
            )
        ),
    CONSTRAINT canonical_pre_interest_consent_event_digest_length_check
        CHECK (octet_length(event_digest) = 32)
);

ALTER TABLE canonical_cloud__interest.canonical_pre_interest_consent_event
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__interest.canonical_pre_interest_consent_event
    FORCE ROW LEVEL SECURITY;

CREATE TABLE canonical_cloud__interest.canonical_pre_interest_outbox (
    outbox_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    submission_id uuid NOT NULL,
    event_type text NOT NULL,
    payload_json jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    published_at timestamptz,
    attempt_count integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz,
    CONSTRAINT canonical_pre_interest_outbox_submission_fk
        FOREIGN KEY (submission_id)
        REFERENCES canonical_cloud__interest.canonical_pre_interest_submission (
            submission_id
        ),
    CONSTRAINT canonical_pre_interest_outbox_event_type_check
        CHECK (event_type IN ('pre_interest.accepted.v1')),
    CONSTRAINT canonical_pre_interest_outbox_payload_object_check
        CHECK (jsonb_typeof(payload_json) = 'object'),
    CONSTRAINT canonical_pre_interest_outbox_attempt_count_check
        CHECK (attempt_count BETWEEN 0 AND 20)
);

ALTER TABLE canonical_cloud__interest.canonical_pre_interest_outbox
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__interest.canonical_pre_interest_outbox
    FORCE ROW LEVEL SECURITY;
