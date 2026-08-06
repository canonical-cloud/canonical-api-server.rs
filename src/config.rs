use std::{env, str::FromStr, time::Duration};

use anyhow::Context;

const DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-pro";
const DEFAULT_QUOTE_CONTEXT_KEY: &str = "quote-analysis";

#[derive(Clone)]
pub struct Config {
    pub(crate) port: u16,
    pub(crate) database_url: String,
    pub(crate) database_max_connections: u32,
    pub(crate) supabase_url: String,
    pub(crate) supabase_publishable_key: String,
    pub(crate) web_service_token: String,
    pub(crate) gemini_api_key: String,
    pub(crate) gemini_model: String,
    pub(crate) quote_context_key: String,
    pub(crate) worker_concurrency: usize,
    pub(crate) worker_poll_interval: Duration,
    pub(crate) worker_lease: Duration,
    pub(crate) worker_max_attempts: i32,
    pub(crate) websocket_max_connections: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("port", &self.port)
            .field("database_url", &"[REDACTED]")
            .field("database_max_connections", &self.database_max_connections)
            .field("supabase_url", &self.supabase_url)
            .field("supabase_publishable_key", &"[REDACTED]")
            .field("web_service_token", &"[REDACTED]")
            .field("gemini_api_key", &"[REDACTED]")
            .field("gemini_model", &self.gemini_model)
            .field("quote_context_key", &self.quote_context_key)
            .field("worker_concurrency", &self.worker_concurrency)
            .field("worker_poll_interval", &self.worker_poll_interval)
            .field("worker_lease", &self.worker_lease)
            .field("worker_max_attempts", &self.worker_max_attempts)
            .field("websocket_max_connections", &self.websocket_max_connections)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = required("DATABASE_URL")?;
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            anyhow::bail!("DATABASE_URL must be a PostgreSQL URL");
        }
        let supabase_url = validated_http_base("SUPABASE_URL", &required("SUPABASE_URL")?)?;
        let supabase_publishable_key = required("SUPABASE_PUBLISHABLE_KEY")?;
        if supabase_publishable_key.starts_with("sb_secret_") || supabase_publishable_key.len() < 20
        {
            anyhow::bail!("SUPABASE_PUBLISHABLE_KEY must be a non-privileged publishable/anon key");
        }
        let web_service_token = required("CANONICAL_WEB_SERVICE_TOKEN")?;
        if web_service_token.len() < 32 || web_service_token.trim() != web_service_token {
            anyhow::bail!(
                "CANONICAL_WEB_SERVICE_TOKEN must contain at least 32 non-whitespace-trimmed bytes"
            );
        }
        let gemini_api_key = required("GEMINI_API_KEY")?;
        if gemini_api_key.len() < 20 || gemini_api_key.trim() != gemini_api_key {
            anyhow::bail!("GEMINI_API_KEY is invalid");
        }
        let gemini_model = match env::var("GEMINI_MODEL") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => DEFAULT_GEMINI_MODEL.to_owned(),
            Err(error) => return Err(error).context("reading GEMINI_MODEL"),
        };
        if gemini_model.is_empty()
            || gemini_model.len() > 128
            || !gemini_model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            anyhow::bail!("GEMINI_MODEL must be a simple model identifier");
        }
        let quote_context_key =
            env::var("QUOTE_CONTEXT_KEY").unwrap_or_else(|_| DEFAULT_QUOTE_CONTEXT_KEY.to_owned());
        if quote_context_key.is_empty()
            || quote_context_key.len() > 128
            || !quote_context_key.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            anyhow::bail!("QUOTE_CONTEXT_KEY must be lowercase URL-safe text");
        }

        let database_max_connections = optional_parse("DATABASE_MAX_CONNECTIONS", 10_u32)?;
        if !(2..=100).contains(&database_max_connections) {
            anyhow::bail!("DATABASE_MAX_CONNECTIONS must be between 2 and 100");
        }
        let worker_concurrency = optional_parse("QUOTE_WORKER_CONCURRENCY", 2_usize)?;
        if !(1..=16).contains(&worker_concurrency) {
            anyhow::bail!("QUOTE_WORKER_CONCURRENCY must be between 1 and 16");
        }
        let poll_ms = optional_parse("QUOTE_WORKER_POLL_MS", 1_000_u64)?;
        if !(100..=60_000).contains(&poll_ms) {
            anyhow::bail!("QUOTE_WORKER_POLL_MS must be between 100 and 60000");
        }
        let lease_seconds = optional_parse("QUOTE_WORKER_LEASE_SECONDS", 300_u64)?;
        if !(30..=1_800).contains(&lease_seconds) {
            anyhow::bail!("QUOTE_WORKER_LEASE_SECONDS must be between 30 and 1800");
        }
        let worker_max_attempts = optional_parse("QUOTE_WORKER_MAX_ATTEMPTS", 3_i32)?;
        if !(1..=10).contains(&worker_max_attempts) {
            anyhow::bail!("QUOTE_WORKER_MAX_ATTEMPTS must be between 1 and 10");
        }
        let websocket_max_connections = optional_parse("WEBSOCKET_MAX_CONNECTIONS", 1_024_usize)?;
        if !(1..=16_384).contains(&websocket_max_connections) {
            anyhow::bail!("WEBSOCKET_MAX_CONNECTIONS must be between 1 and 16384");
        }

        Ok(Self {
            port: optional_parse("PORT", 8080_u16)?,
            database_url,
            database_max_connections,
            supabase_url,
            supabase_publishable_key,
            web_service_token,
            gemini_api_key,
            gemini_model,
            quote_context_key,
            worker_concurrency,
            worker_poll_interval: Duration::from_millis(poll_ms),
            worker_lease: Duration::from_secs(lease_seconds),
            worker_max_attempts,
            websocket_max_connections,
        })
    }
}

fn required(name: &'static str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("{name} is required"))
}

fn optional_parse<T>(name: &'static str, default: T) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("{name} has an invalid value: {error}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("reading {name}")),
    }
}

fn validated_http_base(name: &'static str, value: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(value).with_context(|| format!("parsing {name}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("{name} requires a host"))?;
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.scheme(), "https" | "http")
        || (parsed.scheme() == "http" && !loopback)
    {
        anyhow::bail!("{name} must be HTTPS without credentials, query, or fragment");
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_GEMINI_MODEL, DEFAULT_QUOTE_CONTEXT_KEY};

    #[test]
    fn deployment_defaults_stay_aligned_with_the_canonical_quote_contract() {
        assert_eq!(DEFAULT_GEMINI_MODEL, "gemini-3.6-pro");
        assert_eq!(DEFAULT_QUOTE_CONTEXT_KEY, "quote-analysis");
    }
}
