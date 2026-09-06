use axum::routing::get;
use axum::{Json, Router};
use canonical_interfaces::{HealthStatus, HealthStatusStatus};

pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> Json<HealthStatus> {
    Json(HealthStatus {
        status: HealthStatusStatus::Ok,
        service: "canonical-api-server".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::healthz;
    use canonical_interfaces::HealthStatusStatus;

    #[tokio::test]
    async fn healthz_uses_the_generated_interface_contract() {
        let response = healthz().await.0;
        assert_eq!(response.status, HealthStatusStatus::Ok);
        assert_eq!(response.service, "canonical-api-server");
    }
}
