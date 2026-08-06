BEGIN;

CREATE TABLE IF NOT EXISTS canonical_context (
    id uuid PRIMARY KEY,
    context_key text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    markdown text NOT NULL CHECK (length(markdown) BETWEEN 1 AND 200000),
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    active boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (context_key, version),
    CHECK (context_key ~ '^[a-z0-9][a-z0-9_-]{0,127}$')
);

CREATE UNIQUE INDEX IF NOT EXISTS canonical_context_one_active_key_idx
    ON canonical_context (context_key)
    WHERE active;

CREATE TABLE IF NOT EXISTS quote_request (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL,
    user_email text NOT NULL DEFAULT '' CHECK (length(user_email) <= 320),
    company_name text NOT NULL CHECK (length(company_name) BETWEEN 1 AND 200),
    frameworks jsonb NOT NULL CHECK (jsonb_typeof(frameworks) = 'array'),
    answers jsonb NOT NULL CHECK (jsonb_typeof(answers) = 'object'),
    status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
    analysis jsonb CHECK (analysis IS NULL OR jsonb_typeof(analysis) = 'object'),
    analysis_markdown text,
    estimated_low bigint CHECK (estimated_low IS NULL OR estimated_low >= 0),
    estimated_high bigint CHECK (estimated_high IS NULL OR estimated_high >= 0),
    currency text CHECK (currency IS NULL OR currency = 'USD'),
    gemini_model text CHECK (gemini_model IS NULL OR length(gemini_model) BETWEEN 1 AND 128),
    context_key text NOT NULL CHECK (length(context_key) BETWEEN 1 AND 128),
    context_version bigint CHECK (context_version IS NULL OR context_version > 0),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 100),
    lease_expires_at timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_error text CHECK (last_error IS NULL OR length(last_error) <= 64),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK (estimated_low IS NULL OR estimated_high IS NULL OR estimated_high >= estimated_low),
    CHECK ((status = 'ready') = (analysis IS NOT NULL AND analysis_markdown IS NOT NULL)),
    CHECK (completed_at IS NULL OR completed_at >= created_at)
);

CREATE INDEX IF NOT EXISTS quote_request_user_created_idx
    ON quote_request (user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS quote_request_queue_idx
    ON quote_request (next_attempt_at, created_at)
    WHERE status IN ('queued', 'analyzing');

COMMIT;
