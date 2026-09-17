use axum::Router;
use canonical_api_server::AppState;

pub(super) mod handlers;
pub(super) mod rpc;
mod route;

pub(super) fn router() -> Router<AppState> {
    route::router()
}
