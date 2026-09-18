pub(crate) mod find_user_by_id;
pub(crate) mod find_users;
mod rpc;
pub(crate) mod user;
pub(crate) mod version;

use axum::Router;
use canonical_api_server::AppState;

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .merge(version::router())
        .merge(find_users::router())
        .merge(find_user_by_id::router())
        .with_state(state.clone())
        .merge(rpc::router(state))
}
