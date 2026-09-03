//! Explicit shared HTTP middleware composition for the Canonical API.
//!
//! The existing `shutdown::serve` lifecycle remains authoritative. This module
//! wraps only the Axum router, making the shared request order visible while
//! forcing the shared rate-limit decision into audit-only mode.

use axum::Router;
use ores_middleware::{MiddlewareStage, DEFAULT_MIDDLEWARE_ORDER};

/// Observable inbound and outbound execution order for the shared boundary.
pub const MIDDLEWARE_ORDER: [MiddlewareStage; 16] = DEFAULT_MIDDLEWARE_ORDER;

/// Install the shared request lifecycle without consuming or granting a quota.
pub fn install(router: Router) -> Result<Router, ores_middleware::BootstrapError> {
    ores_middleware::frameworks::axum_audit::install_from_env(router, env!("CARGO_PKG_NAME"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ores_middleware::{
        rate_limit_posture, validate_middleware_order, OperationClass, RateLimitConsistency,
        RateLimitFailureMode,
    };

    #[test]
    fn reviewed_order_is_complete_and_rate_limiting_follows_authentication() {
        assert!(validate_middleware_order(&MIDDLEWARE_ORDER).is_empty());
        assert_eq!(MIDDLEWARE_ORDER[8], MiddlewareStage::Authentication);
        assert_eq!(MIDDLEWARE_ORDER[9], MiddlewareStage::PrincipalRateLimit);
        assert_eq!(MIDDLEWARE_ORDER[10], MiddlewareStage::Authorization);
    }

    #[test]
    fn sensitive_write_classes_require_strict_coordination() {
        for class in [
            OperationClass::AuthAttempt,
            OperationClass::AuthRecovery,
            OperationClass::PaymentWrite,
            OperationClass::LedgerWrite,
        ] {
            let posture = rate_limit_posture(class);
            assert_eq!(posture.consistency, RateLimitConsistency::Strict);
            assert!(posture.coordinator_required);
            assert_ne!(posture.failure_mode, RateLimitFailureMode::FailOpen);
        }
    }
}
