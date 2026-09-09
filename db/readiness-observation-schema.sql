CREATE SCHEMA canonical_cloud__readiness;

CREATE TABLE canonical_cloud__readiness.observation (
    receipt_id uuid PRIMARY KEY,
    event_id text NOT NULL CHECK (char_length(event_id) BETWEEN 1 AND 128),
    owner_subject text NOT NULL CHECK (char_length(owner_subject) BETWEEN 1 AND 255),
    source_id text NOT NULL CHECK (char_length(source_id) BETWEEN 1 AND 128),
    source_sequence bigint NOT NULL CHECK (source_sequence > 0),
    organization text NOT NULL CHECK (char_length(organization) BETWEEN 1 AND 128),
    payload_sha256 text NOT NULL CHECK (
        payload_sha256 ~ '^sha256:[0-9a-f]{64}$'
    ),
    event_json jsonb NOT NULL,
    observed_at timestamptz NOT NULL,
    received_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    substantive_review text NOT NULL DEFAULT 'unreviewed' CHECK (
        substantive_review IN (
            'unreviewed',
            'in-review',
            'accepted-as-evidence',
            'rejected',
            'superseded'
        )
    ),
    CONSTRAINT readiness_observation_event_json_object_check
        CHECK (jsonb_typeof(event_json) = 'object'),
    CONSTRAINT readiness_observation_event_owner_unique
        UNIQUE (owner_subject, event_id),
    CONSTRAINT readiness_observation_source_sequence_unique
        UNIQUE (owner_subject, source_id, source_sequence)
);

CREATE INDEX readiness_observation_owner_received_idx
    ON canonical_cloud__readiness.observation (
        owner_subject,
        received_at DESC,
        receipt_id DESC
    );

CREATE INDEX readiness_observation_source_sequence_idx
    ON canonical_cloud__readiness.observation (
        owner_subject,
        source_id,
        source_sequence DESC
    );

ALTER TABLE canonical_cloud__readiness.observation
    ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_cloud__readiness.observation
    FORCE ROW LEVEL SECURITY;

CREATE POLICY readiness_observation_owner_policy
ON canonical_cloud__readiness.observation
USING (
    owner_subject = current_setting('app.current_subject', TRUE)
)
WITH CHECK (
    owner_subject = current_setting('app.current_subject', TRUE)
);

COMMENT ON TABLE canonical_cloud__readiness.observation IS
    'Append-only customer readiness observations. Transport acceptance is not substantive evidence acceptance.';
