use std::collections::BTreeSet;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::{ApiError, Secret};

const USER_HOST: &str = "user.canonical.plus";
const ORG_HOST: &str = "org.canonical.plus";
const MAX_EMAIL_BYTES: usize = 254;
const MAX_ORGANIZATION_NAME_CHARS: usize = 200;
const MAX_INTEREST_AREAS: usize = 20;
const MAX_INTEREST_AREA_BYTES: usize = 64;
const MAX_CONSENT_VERSION_BYTES: usize = 64;
const MAX_LOCALE_BYTES: usize = 35;
const MAX_REFERRAL_CODE_BYTES: usize = 64;
const MAX_PROOF_TOKEN_BYTES: usize = 2048;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum PartyType {
    Individual,
    Organization,
}

impl PartyType {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Individual => "individual",
            Self::Organization => "organization",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PreInterestRequest {
    email: String,
    party_type: PartyType,
    #[serde(default)]
    organization_name: Option<String>,
    interest_areas: Vec<String>,
    consent_version: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    referral_code: Option<String>,
    turnstile_token: String,
}

#[derive(Debug)]
pub(super) struct NormalizedRequest {
    consent_version: String,
    email: String,
    interest_areas: Vec<String>,
    locale: Option<String>,
    organization_name: Option<String>,
    party_type: PartyType,
    referral_code: Option<String>,
    pub(super) turnstile_token: String,
}

#[derive(Debug)]
pub(super) struct AcceptedRegistration {
    pub(super) consent_version: String,
    pub(super) email: String,
    pub(super) email_alias: String,
    pub(super) interest_areas: Vec<String>,
    pub(super) locale: Option<String>,
    pub(super) organization_name: Option<String>,
    pub(super) party_type: PartyType,
    pub(super) referral_code: Option<String>,
    pub(super) request_alias: String,
    pub(super) source_host: String,
}

pub(super) fn normalize_request(
    payload: PreInterestRequest,
) -> Result<NormalizedRequest, ApiError> {
    let email = normalize_email(&payload.email)?;
    let organization_name = normalize_organization_name(
        payload.party_type,
        payload.organization_name.as_deref(),
    )?;
    let interest_areas = normalize_interest_areas(payload.interest_areas)?;
    let consent_version = portable_identifier(
        &payload.consent_version,
        MAX_CONSENT_VERSION_BYTES,
    )?;
    let locale = payload
        .locale
        .as_deref()
        .map(normalize_locale)
        .transpose()?;
    let referral_code = payload
        .referral_code
        .as_deref()
        .map(|value| portable_identifier(value, MAX_REFERRAL_CODE_BYTES))
        .transpose()?;
    let turnstile_token = payload.turnstile_token;
    if turnstile_token.is_empty()
        || turnstile_token.len() > MAX_PROOF_TOKEN_BYTES
        || turnstile_token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(ApiError::InvalidRequest);
    }

    Ok(NormalizedRequest {
        consent_version,
        email,
        interest_areas,
        locale,
        organization_name,
        party_type: payload.party_type,
        referral_code,
        turnstile_token,
    })
}

pub(super) fn accept_request(
    normalized: NormalizedRequest,
    verified_host: String,
    hmac_key: &Secret,
) -> Result<AcceptedRegistration, ApiError> {
    let expected_party = match verified_host.as_str() {
        USER_HOST => PartyType::Individual,
        ORG_HOST => PartyType::Organization,
        _ => return Err(ApiError::InvalidRequest),
    };
    if normalized.party_type != expected_party {
        return Err(ApiError::InvalidRequest);
    }

    let email_alias = hmac_alias(hmac_key, "email", &[normalized.email.as_str()])?;
    let interest_joined = normalized.interest_areas.join("\u{1f}");
    let request_alias = hmac_alias(
        hmac_key,
        "request",
        &[
            normalized.email.as_str(),
            normalized.party_type.as_str(),
            normalized.organization_name.as_deref().unwrap_or(""),
            interest_joined.as_str(),
            normalized.consent_version.as_str(),
            normalized.locale.as_deref().unwrap_or(""),
            normalized.referral_code.as_deref().unwrap_or(""),
            verified_host.as_str(),
        ],
    )?;

    Ok(AcceptedRegistration {
        consent_version: normalized.consent_version,
        email: normalized.email,
        email_alias,
        interest_areas: normalized.interest_areas,
        locale: normalized.locale,
        organization_name: normalized.organization_name,
        party_type: normalized.party_type,
        referral_code: normalized.referral_code,
        request_alias,
        source_host: verified_host,
    })
}

fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > MAX_EMAIL_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(ApiError::InvalidRequest);
    }
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if local.is_empty()
        || domain.is_empty()
        || parts.next().is_some()
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
        || domain.ends_with('-')
        || !domain.contains('.')
        || !local
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&byte))
        || !domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(format!(
        "{}@{}",
        local.to_ascii_lowercase(),
        domain.to_ascii_lowercase()
    ))
}

fn normalize_organization_name(
    party_type: PartyType,
    raw: Option<&str>,
) -> Result<Option<String>, ApiError> {
    match (party_type, raw.map(str::trim).filter(|value| !value.is_empty())) {
        (PartyType::Individual, None) => Ok(None),
        (PartyType::Individual, Some(_)) => Err(ApiError::InvalidRequest),
        (PartyType::Organization, None) => Err(ApiError::InvalidRequest),
        (PartyType::Organization, Some(value))
            if value.chars().count() <= MAX_ORGANIZATION_NAME_CHARS
                && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value.to_owned()))
        }
        (PartyType::Organization, Some(_)) => Err(ApiError::InvalidRequest),
    }
}

fn normalize_interest_areas(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    if values.is_empty() || values.len() > MAX_INTEREST_AREAS {
        return Err(ApiError::InvalidRequest);
    }
    let mut normalized = BTreeSet::new();
    for value in values {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty()
            || value.len() > MAX_INTEREST_AREA_BYTES
            || !value.is_ascii()
            || !value.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphanumeric()
                    || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
            })
        {
            return Err(ApiError::InvalidRequest);
        }
        normalized.insert(value);
    }
    Ok(normalized.into_iter().collect())
}

fn portable_identifier(raw: &str, max_bytes: usize) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > max_bytes
        || !value.is_ascii()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn normalize_locale(raw: &str) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.len() < 2
        || value.len() > MAX_LOCALE_BYTES
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn hmac_alias(
    key: &Secret,
    domain: &str,
    fields: &[&str],
) -> Result<String, ApiError> {
    let mut mac = HmacSha256::new_from_slice(key.expose().as_bytes())
        .map_err(|_| ApiError::Unavailable)?;
    mac.update(b"canonical.pre-interest.v1\0");
    mac.update(domain.as_bytes());
    mac.update(b"\0");
    for field in fields {
        let length = u64::try_from(field.len()).map_err(|_| ApiError::InvalidRequest)?;
        mac.update(&length.to_be_bytes());
        mac.update(field.as_bytes());
    }
    Ok(hex_lower(&mac.finalize().into_bytes()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hmac_key() -> Secret {
        Secret("0123456789abcdef0123456789abcdef".into())
    }

    fn request(party_type: PartyType) -> PreInterestRequest {
        PreInterestRequest {
            email: " Person@Example.COM ".into(),
            party_type,
            organization_name: (party_type == PartyType::Organization)
                .then(|| " Example Roofing ".into()),
            interest_areas: vec!["SOC2".into(), "security-audit".into(), "soc2".into()],
            consent_version: "pre-interest-v1".into(),
            locale: Some("en-US".into()),
            referral_code: Some("partner-1".into()),
            turnstile_token: "turnstile-test-token".into(),
        }
    }

    #[test]
    fn email_normalization_is_bounded_and_deterministic() {
        assert_eq!(
            normalize_email(" Person@Example.COM ").unwrap(),
            "person@example.com"
        );
        for invalid in [
            "",
            "missing-at.example.com",
            "two@@example.com",
            ".person@example.com",
            "person@example",
            "person@-example.com",
            "person @example.com",
        ] {
            assert!(normalize_email(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn party_type_and_organization_name_must_agree() {
        assert!(normalize_request(request(PartyType::Individual)).is_ok());
        assert!(normalize_request(request(PartyType::Organization)).is_ok());

        let mut individual = request(PartyType::Individual);
        individual.organization_name = Some("Injected Org".into());
        assert!(normalize_request(individual).is_err());

        let mut organization = request(PartyType::Organization);
        organization.organization_name = None;
        assert!(normalize_request(organization).is_err());
    }

    #[test]
    fn interest_areas_are_sorted_deduplicated_and_portable() {
        let normalized = normalize_request(request(PartyType::Individual)).unwrap();
        assert_eq!(normalized.interest_areas, ["security-audit", "soc2"]);

        let mut invalid = request(PartyType::Individual);
        invalid.interest_areas = vec!["not allowed".into()];
        assert!(normalize_request(invalid).is_err());
    }

    #[test]
    fn verified_hostname_must_match_party_type() {
        let normalized = normalize_request(request(PartyType::Individual)).unwrap();
        assert!(accept_request(normalized, USER_HOST.into(), &hmac_key()).is_ok());

        let normalized = normalize_request(request(PartyType::Individual)).unwrap();
        assert!(accept_request(normalized, ORG_HOST.into(), &hmac_key()).is_err());
    }

    #[test]
    fn aliases_are_stable_domain_separated_and_do_not_expose_email() {
        let key = hmac_key();
        let email = hmac_alias(&key, "email", &["person@example.com"]).unwrap();
        let repeated = hmac_alias(&key, "email", &["person@example.com"]).unwrap();
        let request = hmac_alias(&key, "request", &["person@example.com"]).unwrap();
        assert_eq!(email, repeated);
        assert_ne!(email, request);
        assert_eq!(email.len(), 64);
        assert!(!email.contains("person"));
    }
}
