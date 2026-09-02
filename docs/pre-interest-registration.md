# Pre-interest registration foundation

Tracking: **DEN-4058**. Contract source witness:
`canonical-cloud/canonical-interfaces` commit
`e70ca809fb996c8ba2067b13c24938f4243e3784`.

This change deliberately separates pure request handling from network,
database, and Cloudflare effects.

## Implemented in this slice

- Closed Serde request decoding with camelCase wire names.
- Exact `user.canonical.plus` / individual and
  `org.canonical.plus` / organization host-party validation.
- Email, organization, interest, consent, locale, and referral normalization.
- Redacted debug output for contact fields.
- Dedicated HMAC domain separation for email aliases and idempotency request
  fingerprints.
- Stable enumeration-resistant accepted responses.
- A candidate PostgreSQL 17 schema for registration identity, immutable
  submissions and consent events, and a durable outbox.

The candidate SQL is not connected to `db/namespace.json` and must not be
applied to production.

## Required write transaction

The Diesel/diesel-async primary adapter must perform one transaction:

1. Derive the normalized-email alias with the dedicated alias key. Encrypt
   contact values with a separate field-encryption key.
2. Insert or select the registration by `email_alias` without branching the
   public response on whether it already existed.
3. Insert `request_id`, its dedicated HMAC request fingerprint, and a stable
   `receipt_id`. A duplicate request ID with the same fingerprint returns the
   same receipt; a different fingerprint is a conflict.
4. Insert immutable consent evidence and the outbox row in the same
   transaction.
5. Commit before returning the uniform `accepted` response.

The SeaORM adapter is a secondary read/entity projection over the same
reviewed schema. It may not invent columns, table names, or alternate
idempotency behavior. Disposable PostgreSQL tests must execute equivalent
authorized operations through both adapters before either is wired to HTTP.

## HTTP activation gates

`POST /v1/pre-interest-registrations` remains unbound in this foundation.
Before routing it:

- compile and publish the generated contract adapters;
- install a bounded anti-automation verifier and per-alias/per-network rate
  limits;
- derive the request host at the origin after stripping spoofable forwarding
  and classification headers;
- configure separate alias, fingerprint, and contact-encryption keys through
  the approved secret path;
- certify concurrent duplicate behavior on PostgreSQL;
- verify that logs, metrics, traces, errors, URLs, and Cloudflare headers never
  contain raw contact values;
- canary exact source heads in `canonical-cloud-test`.

Registration never creates a session, account, quote, role, grant, or
entitlement. The quote link in the response is an explicit next step only.
