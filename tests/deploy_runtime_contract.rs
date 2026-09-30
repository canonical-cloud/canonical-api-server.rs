#![forbid(unsafe_code)]

const DEPLOY: &str = include_str!("../deploy/k8s/all.yaml");
const MAIN: &str = include_str!("../src/main.rs");
const READINESS: &str = include_str!("../src/readiness.rs");
const FLAGS: &str = include_str!("../.cli-flags.toml");

#[test]
fn kubernetes_listener_matches_runtime_flag_contract() {
    assert!(FLAGS.contains("env = \"BIND_ADDRESS\""));
    assert!(DEPLOY.contains("- name: BIND_ADDRESS\n              value: 0.0.0.0:8081"));
    assert!(!DEPLOY.contains("- name: BIND_ADDR\n"));
    assert!(DEPLOY.contains("containerPort: 8081"));
    assert!(DEPLOY.contains("port: 8081\n      targetPort: http"));
}

#[test]
fn kubernetes_supplies_required_runtime_auth_secret_and_rejects_retired_envs() {
    assert!(FLAGS.contains("CANONICAL_INTERNAL_AUTH_TOKEN"));
    assert!(DEPLOY.contains("- name: CANONICAL_INTERNAL_AUTH_TOKEN"));
    assert!(DEPLOY.contains("key: CANONICAL_INTERNAL_AUTH_TOKEN"));
    assert!(!DEPLOY.contains("QUOTE_CONTEXT_MARKDOWN_PATH"));
    assert!(!DEPLOY.contains("ORIGIN_ASSERTION_SECRET"));
}

#[test]
fn kubernetes_readiness_route_is_real_and_database_backed() {
    assert!(MAIN.contains(".merge(readiness::router(readiness_database))"));
    assert!(DEPLOY.contains("path: /readyz"));
    assert!(READINESS
        .contains("PostgreSQL is required before the Canonical quote API can receive traffic"));
    assert!(READINESS.contains("rolbypassrls"));
    assert!(READINESS.contains("relation.oid IS NULL"));
    assert!(READINESS.contains("constraint_row.convalidated"));
    assert!(!READINESS.contains("count(*) = 7"));
    assert!(!READINESS.contains("count(*) = 6"));
    assert!(!READINESS.contains("count(*) = 32"));
}

#[test]
fn kubernetes_termination_window_exceeds_application_shutdown_grace() {
    assert!(FLAGS.contains("env = \"SHUTDOWN_GRACE_MS\""));
    assert!(DEPLOY.contains("terminationGracePeriodSeconds: 40"));
    assert!(DEPLOY.contains("- name: SHUTDOWN_GRACE_MS\n              value: \"30000\""));
}
