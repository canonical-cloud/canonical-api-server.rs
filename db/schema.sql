create extension if not exists pgcrypto;

create table if not exists public.canonical_context (
    id uuid primary key default gen_random_uuid(),
    context_key text not null,
    content text not null,
    active boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    constraint canonical_context_key_length check (char_length(context_key) between 1 and 128),
    constraint canonical_context_content_length check (octet_length(content) <= 262144)
);

create unique index if not exists canonical_context_one_active_key
    on public.canonical_context (context_key)
    where active;

create table if not exists public.quote_request (
    id uuid primary key default gen_random_uuid(),
    owner_id uuid not null,
    owner_email text,
    status text not null,
    intake jsonb not null,
    analysis jsonb,
    model text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    constraint quote_request_status check (status in ('analyzing', 'complete', 'failed')),
    constraint quote_request_owner_email_length check (owner_email is null or char_length(owner_email) <= 320)
);

create index if not exists quote_request_owner_created
    on public.quote_request (owner_id, created_at desc);

alter table public.canonical_context enable row level security;
alter table public.quote_request enable row level security;

comment on table public.canonical_context is
    'Operator-managed context appended to the versioned quote analysis playbook.';
comment on table public.quote_request is
    'Canonical compliance quote requests and their structured Gemini analyses.';
