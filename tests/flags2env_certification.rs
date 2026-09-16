const FLAGS_SOURCE: &str = include_str!("../src/flags.rs");
const CONTRACT: &str = include_str!("../.cli-flags.toml");

#[test]
fn parser_and_contract_are_bundled_into_the_server_artifact() {
    assert!(FLAGS_SOURCE.contains("BundledFlags2Env"));
    assert!(FLAGS_SOURCE.contains("include_str!(\"../.cli-flags.toml\")"));
    assert!(FLAGS_SOURCE.contains("OnceLock"));
}

#[test]
fn readiness_credentials_remain_environment_only_and_unknown_flags_fail_closed() {
    assert!(CONTRACT.contains("allow_unknown = false"));
    for secret in ["DATABASE_URL", "CANONICAL_WEBHOOK_SECRET", "CANONICAL_INTERNAL_AUTH_TOKEN", "GEMINI_API_KEY", "SHARED_AUTH_INTROSPECT_SECRET"] {
        assert!(CONTRACT.contains(secret), "missing environment-only credential {secret}");
        assert!(!CONTRACT.contains(&format!("long = \"{}\"", secret.to_ascii_lowercase().replace('_', "-"))));
    }
}

#[test]
fn argv_errors_are_value_safe_and_cli_values_win_over_environment() {
    assert!(FLAGS_SOURCE.contains("option_name(option)"));
    assert!(FLAGS_SOURCE.contains("parsed.provided_flags"));
    assert!(FLAGS_SOURCE.contains("unknown_options_fail_closed_without_echoing_values"));
    assert!(FLAGS_SOURCE.contains("command_line_overrides_environment_with_a_typed_value"));
}
