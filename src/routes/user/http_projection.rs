//! Shared HTTP-projection helpers for the user lookup operations.
//!
//! `handlers.rs` remains the semantic authority. The two operations it owns are
//! projected onto distinct URLs (`/v1/find-users`, `/v1/find-user-by-id`), and
//! the filesystem route contract gives each URL its own folder, so this module
//! holds what both projections need rather than duplicating it in each.

use axum::http::HeaderMap;

use super::handlers::UserLookupHeaders;

pub(crate) fn request_headers(headers: &HeaderMap) -> UserLookupHeaders {
    UserLookupHeaders {
        authorization: headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    }
}
