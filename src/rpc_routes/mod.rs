mod rpc;
pub(crate) mod user;
pub(crate) mod version;

use axum::Router;
use canonical_api_server::AppState;

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .merge(version::router())
        .merge(user::route::find_users::router())
        .merge(user::route::find_user_by_id::router())
        .with_state(state.clone())
        .merge(rpc::router(state))
}
