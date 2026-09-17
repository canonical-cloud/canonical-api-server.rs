mod rpc;
pub(super) mod version;

use axum::Router;
use canonical_api_server::AppState;

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .merge(version::router())
        .with_state(state.clone())
        .merge(rpc::router(state))
}
