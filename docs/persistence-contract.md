# Persistence contract

The API process consumes two physically separate persistence planes:

1. `CANONICAL_AUDIT_DATABASE_URL` is the customer audit domain and is exposed to
   this server only through `canonical-orm-core` capability verification. No raw
   audit driver handle is retained in application state.
2. `DATABASE_URL` is the quote/readiness plane. Its PostgreSQL realization and
   runtime store are owned by `canonical-cloud/canonical-orm-core`; this server
   owns transport, authentication, orchestration, and protocol responses only.

The two URLs must not be the same credential or database trust plane.

## Quote/readiness durable records

- `canonical_context`: owner-scoped operational context selected by the API;
- `canonical_quote`: immutable normalized request, application Markdown,
  database-context snapshots, chosen model, status, structured analysis, and a
  bounded error code;
- `canonical_model_attempt`: provider/model and start/finish metadata only;
- `canonical_quote_event`: append-only status sequence used to recover the
  state represented by REST and WebSocket responses;
- `canonical_readiness_observation`: signed-observation receipt stream with
  per-source contiguous sequencing and a domain-separated record hash chain.

Raw API keys, internal service tokens, and provider error bodies are never
stored. Application logs contain bounded identifiers/error codes, not prompts,
context, model output, database credentials, or user secrets.

## Ownership and transactions

`canonical-orm-core::QuoteStore` is the opaque runtime persistence boundary. It
owns the SeaORM and Diesel pools and fails closed during construction unless the
exact quote runtime role, search path, object ownership, forced RLS, policies,
constraints, indexes, and grants are independently witnessed through both
ORMs. Raw quote/readiness driver handles do not belong in API application state.

Named quote operations and readiness-observation append operations execute
inside orm-core. Observation append owns the advisory stream lock, event-id
idempotency, contiguous sequence check, previous-record lookup, hash-chain
construction, and immutable insert.

Every owner-scoped lookup or mutation installs the validated subject in
`app.current_subject`, applies explicit owner predicates, and executes against
forced-RLS tables. WebSocket notifications are disposable hints; durable state
is always recovered from PostgreSQL.

## DDL and migration ownership

The authoritative declarative quote persistence sources are in
`canonical-cloud/canonical-orm-core`:

- `sql/quote/bootstrap.sql`
- `sql/quote/schema.sql`
- `sql/quote/grants.sql`
- `sql/quote-readiness.sql`

Infrastructure provisions the database/network/secrets and executes approved
migration/declarative deployment procedures; it does not own the domain DDL.

During the DEN-3938 transition, this repository's
`db/{bootstrap,schema,grants}.sql` files remain **frozen compatibility witnesses**
for existing declarative-postgres CI. They are not a second editable authority.
`db/namespace.json` records the orm-core source location and transition source
commit. The follow-up consumer/deployment cutover should materialize those files
from orm-core or remove them when CI/deployment reads orm-core directly.

Future production quote-schema changes originate in `canonical-orm-core` and
use reviewed forward migration/declarative migration planning there. They do not
originate in this API repository or in `canonical-infra`.
