use axum::{routing::get, Json, Router};
use serde::Serialize;

use canonical_api_server::AppState;

const CONTRACT: &str = "canonical-cloud/grc-platform/v1";

#[derive(Clone, Copy, Debug, Serialize)]
struct GrcCapabilityDescriptor {
    key: &'static str,
    api_path: &'static str,
    app_path: &'static str,
    persistence_tables: &'static [&'static str],
    continuous_monitoring: bool,
    evidence_provenance: bool,
    human_approval: bool,
}

#[derive(Debug, Serialize)]
struct GrcCapabilityCatalog {
    contract: &'static str,
    schema_version: u8,
    capabilities: &'static [GrcCapabilityDescriptor],
}

const CAPABILITIES: &[GrcCapabilityDescriptor] = &[
    GrcCapabilityDescriptor {
        key: "controls_evidence",
        api_path: "/v1/grc/controls",
        app_path: "/app/compliance/controls",
        persistence_tables: &[
            "control_implementations",
            "control_exceptions",
            "control_tests",
            "control_test_runs",
            "evidence_records",
            "evidence_control_links",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "framework_crosswalks",
        api_path: "/v1/grc/frameworks",
        app_path: "/app/compliance/frameworks",
        persistence_tables: &[
            "framework_releases",
            "control_references",
            "audit_engagement_frameworks",
            "framework_coverage_snapshots",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "policies_people_training",
        api_path: "/v1/grc/policies",
        app_path: "/app/compliance/policies",
        persistence_tables: &[
            "policies",
            "policy_versions",
            "policy_acknowledgements",
            "personnel_records",
            "training_courses",
            "training_assignments",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "risk_management",
        api_path: "/v1/grc/risks",
        app_path: "/app/compliance/risks",
        persistence_tables: &["risks", "risk_controls", "risk_treatments"],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "asset_vulnerability",
        api_path: "/v1/grc/assets",
        app_path: "/app/compliance/assets",
        persistence_tables: &["assets", "asset_relationships", "vulnerabilities"],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: false,
    },
    GrcCapabilityDescriptor {
        key: "vendor_risk",
        api_path: "/v1/grc/vendors",
        app_path: "/app/compliance/vendors",
        persistence_tables: &[
            "vendors",
            "vendor_assessments",
            "vendor_assessment_evidence",
            "vendor_monitor_events",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "access_reviews",
        api_path: "/v1/grc/access-reviews",
        app_path: "/app/compliance/access-reviews",
        persistence_tables: &["access_review_campaigns", "access_review_items"],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "audit_collaboration",
        api_path: "/v1/grc/audits",
        app_path: "/app/compliance/audits",
        persistence_tables: &[
            "audit_engagements",
            "audit_phases",
            "audit_evidence_requests",
            "audit_evidence_request_items",
            "findings",
            "recommendations",
        ],
        continuous_monitoring: false,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "trust_center",
        api_path: "/v1/grc/trust-centers",
        app_path: "/app/compliance/trust-center",
        persistence_tables: &[
            "trust_centers",
            "trust_center_resources",
            "trust_center_access_requests",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "security_questionnaires",
        api_path: "/v1/grc/questionnaires",
        app_path: "/app/compliance/questionnaires",
        persistence_tables: &[
            "security_questionnaires",
            "security_questionnaire_questions",
            "security_questionnaire_answers",
            "questionnaire_answer_sources",
            "knowledge_base_entries",
        ],
        continuous_monitoring: false,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "integrations_automation",
        api_path: "/v1/grc/automations",
        app_path: "/app/compliance/automations",
        persistence_tables: &[
            "integration_connections",
            "connection_permission_snapshots",
            "automation_jobs",
            "automation_runs",
        ],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: false,
    },
    GrcCapabilityDescriptor {
        key: "ai_governance",
        api_path: "/v1/grc/ai-systems",
        app_path: "/app/compliance/ai-governance",
        persistence_tables: &["ai_systems", "ai_system_controls", "ai_monitor_events"],
        continuous_monitoring: true,
        evidence_provenance: true,
        human_approval: true,
    },
    GrcCapabilityDescriptor {
        key: "commercial_entitlements",
        api_path: "/v1/grc/subscription",
        app_path: "/app/compliance/subscription",
        persistence_tables: &[
            "commercial_plan_catalog",
            "commercial_plan_features",
            "tenant_subscriptions",
        ],
        continuous_monitoring: false,
        evidence_provenance: true,
        human_approval: false,
    },
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/grc/capabilities", get(capabilities))
        .route("/api/v1/grc/capabilities", get(capabilities))
}

async fn capabilities() -> Json<GrcCapabilityCatalog> {
    Json(GrcCapabilityCatalog {
        contract: CONTRACT,
        schema_version: 1,
        capabilities: CAPABILITIES,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{CAPABILITIES, CONTRACT};

    #[test]
    fn capability_catalog_is_complete_bounded_and_non_generic() {
        assert_eq!(CONTRACT, "canonical-cloud/grc-platform/v1");
        assert_eq!(CAPABILITIES.len(), 13);

        let keys = CAPABILITIES
            .iter()
            .map(|capability| capability.key)
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), CAPABILITIES.len());

        for capability in CAPABILITIES {
            assert!(capability.api_path.starts_with("/v1/grc/"));
            assert!(capability.app_path.starts_with("/app/compliance/"));
            assert!(!capability.persistence_tables.is_empty());
            assert!(!capability.persistence_tables.contains(&"arbitrary_sql"));
            assert!(!capability.persistence_tables.contains(&"everything"));
        }
    }
}
