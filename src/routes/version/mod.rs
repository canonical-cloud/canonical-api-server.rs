use axum::Router;
use canonical_api_server::AppState;

pub(crate) mod handlers;
pub(crate) mod route;
pub(crate) mod rpc;

pub(crate) fn router() -> Router<AppState> {
    route::router()
}
