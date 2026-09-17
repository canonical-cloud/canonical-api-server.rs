mod rpc;
pub(super) mod user;
pub(super) mod version;

use axum::Router;
use canonical_api_server::AppState;

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .merge(version::router())
        .merge(user::router())
        .with_state(state.clone())
        .merge(rpc::router(state))
}
