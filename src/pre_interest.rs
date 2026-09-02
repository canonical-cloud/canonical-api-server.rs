use std::collections::BTreeSet;
use std::fmt;

use hmac::{Hmac, Mac};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const MAX_EMAIL_BYTES: usize = 320;
const MAX_ORGANIZATION_CHARS: usize = 200;
const MAX_DISPLAY_NAME_CHARS: usize = 120;
const MAX_WEBSITE_URL_BYTES: usize = 2_048;
const MAX_INTERESTS: usize = 9;
const MAX_CONSENT_REVISION_BYTES: usize = 64;
const MAX_MARKETING_CONSENT_REVISION_BYTES: usize = 64;
const MAX_LOCALE_BYTES: usize = 35;
const MAX_REFERRAL_BYTES: usize = 64;
const MIN_DERIVATION_KEY_BYTES: usize = 32;
const EMAIL_ALIAS_CONTEXT: &[u8] = b"canonical.pre-interest.email-alias.v1\0";
const REQUEST_FINGERPRINT_CONTEXT: &[u8] =
    b"canonical.pre-interest.request-fingerprint.v1\0";
const QUOTE_NEXT_STEP_URL: &str = "https://user.canonical.plus/u/quote";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyType {
    Individual,
    Organization,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum InterestArea {
    #[serde(rename = "readiness_assessment")]
    ReadinessAssessment,
    #[serde(rename = "soc2")]
    Soc2,
    #[serde(rename = "iso_27001")]
    Iso27001,
    #[serde(rename = "hipaa")]
    Hipaa,
    #[serde(rename = "pci_dss_4")]
    PciDss4,
    #[serde(rename = "fedramp")]
    Fedramp,
    #[serde(rename = "nist")]
    Nist,
    #[serde(rename = "gdpr")]
    Gdpr,
    #[serde(rename = "cmmc")]
    Cmmc,
}

impl InterestArea {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadinessAssessment => "readiness_assessment",
            Self::Soc2 => "soc2",
            Self::Iso27001 => "iso_27001",
            Self::Hipaa => "hipaa",
            Self::PciDss4 => "pci_dss_4",
            Self::Fedramp => "fedramp",
            Self::Nist => "nist",
            Self::Gdpr => "gdpr",
            Self::Cmmc => "cmmc",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreInterestRegistrationRequest {
    pub request_id: Uuid,
    pub email: String,
    pub party_type: PartyType,
    pub organization_name: Option<String>,
    pub interest_areas: Vec<InterestArea>,
    pub consent_revision: String,
    pub consented_at: String,
    pub source_host: String,
    pub locale: Option<String>,
    pub referral_code: Option<String>,
    pub display_name: Option<String>,
    pub website_url: Option<String>,
    pub registration_consent: bool,
    pub marketing_consent: bool,
    pub marketing_consent_revision: Option<String>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SensitiveText(String);

impl SensitiveText {
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationHost {
    User,
    Organization,
}

impl RegistrationHost {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        match value {
            "user.canonical.plus" => Ok(Self::User),
            "org.canonical.plus" => Ok(Self::Organization),
            _ => Err(ValidationError::InvalidSourceHost),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user.canonical.plus",
            Self::Organization => "org.canonical.plus",
        }
    }

    #[must_use]
    pub const fn party_type(self) -> PartyType {
        match self {
            Self::User => PartyType::Individual,
            Self::Organization => PartyType::Organization,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedPreInterestRegistration {
    request_id: Uuid,
    email: SensitiveText,
    party_type: PartyType,
    organization_name: Option<SensitiveText>,
    interest_areas: Vec<InterestArea>,
    consent_revision: String,
    consented_at: String,
    source_host: RegistrationHost,
    locale: Option<String>,
    referral_code: Option<SensitiveText>,
    display_name: Option<SensitiveText>,
    website_url: Option<SensitiveText>,
    registration_consent: bool,
    marketing_consent: bool,
    marketing_consent_revision: Option<String>,
}

impl ValidatedPreInterestRegistration {
    #[must_use]
    pub fn request_id(&self) -> &Uuid {
        &self.request_id
    }

    #[must_use]
    pub const fn party_type(&self) -> PartyType {
        self.party_type
    }

    #[must_use]
    pub const fn source_host(&self) -> RegistrationHost {
        self.source_host
    }

    #[must_use]
    pub fn normalized_email(&self) -> &SensitiveText {
        &self.email
    }

    #[must_use]
    pub fn organization_name(&self) -> Option<&SensitiveText> {
        self.organization_name.as_ref()
    }

    #[must_use]
    pub fn interest_areas(&self) -> &[InterestArea] {
        &self.interest_areas
    }

    #[must_use]
    pub fn consent_revision(&self) -> &str {
        &self.consent_revision
    }

    #[must_use]
    pub fn consented_at(&self) -> &str {
        &self.consented_at
    }

    #[must_use]
    pub fn locale(&self) -> Option<&str> {
        self.locale.as_deref()
    }

    #[must_use]
    pub fn referral_code(&self) -> Option<&SensitiveText> {
        self.referral_code.as_ref()
    }

    #[must_use]
    pub fn display_name(&self) -> Option<&SensitiveText> {
        self.display_name.as_ref()
    }

    #[must_use]
    pub fn website_url(&self) -> Option<&SensitiveText> {
        self.website_url.as_ref()
    }

    #[must_use]
    pub const fn registration_consent(&self) -> bool {
        self.registration_consent
    }

    #[must_use]
    pub const fn marketing_consent(&self) -> bool {
        self.marketing_consent
    }

    #[must_use]
    pub fn marketing_consent_revision(&self) -> Option<&str> {
        self.marketing_consent_revision.as_deref()
    }
}

impl fmt::Debug for ValidatedPreInterestRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedPreInterestRegistration")
            .field("request_id", &self.request_id)
            .field("email", &"[redacted]")
            .field("party_type", &self.party_type)
            .field(
                "organization_name_present",
                &self.organization_name.is_some(),
            )
            .field("interest_areas", &self.interest_areas)
            .field("consent_revision", &self.consent_revision)
            .field("consented_at", &self.consented_at)
            .field("source_host", &self.source_host)
            .field("locale", &self.locale)
            .field("referral_code_present", &self.referral_code.is_some())
            .field("display_name_present", &self.display_name.is_some())
            .field("website_url_present", &self.website_url.is_some())
            .field("registration_consent", &self.registration_consent)
            .field("marketing_consent", &self.marketing_consent)
            .field(
                "marketing_consent_revision_present",
                &self.marketing_consent_revision.is_some(),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OpaqueDigest([u8; 32]);

impl OpaqueDigest {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

impl fmt::Debug for OpaqueDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[opaque-digest]")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    DuplicateInterest,
    EmptyInterestSet,
    InvalidConsentRevision,
    InvalidConsentedAt,
    InvalidDisplayName,
    InvalidEmail,
    InvalidLocale,
    InvalidMarketingConsentRevision,
    InvalidOrganizationName,
    InvalidReferralCode,
    InvalidSourceHost,
    InvalidWebsiteUrl,
    OrganizationNameForbidden,
    OrganizationNameRequired,
    PartyHostMismatch,
    RegistrationConsentRequired,
    SourceHostMismatch,
    TooManyInterests,
    UnexpectedMarketingConsentRevision,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateInterest => "interest areas must be unique",
            Self::EmptyInterestSet => "at least one interest area is required",
            Self::InvalidConsentRevision => "consent revision is invalid",
            Self::InvalidConsentedAt => "consent timestamp is invalid",
            Self::InvalidDisplayName => "display name is invalid",
            Self::InvalidEmail => "email is invalid",
            Self::InvalidLocale => "locale is invalid",
            Self::InvalidMarketingConsentRevision => {
                "marketing consent revision is invalid"
            }
            Self::InvalidOrganizationName => "organization name is invalid",
            Self::InvalidReferralCode => "referral code is invalid",
            Self::InvalidSourceHost => "registration host is invalid",
            Self::InvalidWebsiteUrl => "website URL is invalid",
            Self::OrganizationNameForbidden => {
                "organization name is forbidden for individual registration"
            }
            Self::OrganizationNameRequired => {
                "organization name is required for organization registration"
            }
            Self::PartyHostMismatch => "registration host and party type do not agree",
            Self::RegistrationConsentRequired => {
                "registration consent must be affirmatively granted"
            }
            Self::SourceHostMismatch => {
                "request source host does not match the verified request host"
            }
            Self::TooManyInterests => "too many interest areas were supplied",
            Self::UnexpectedMarketingConsentRevision => {
                "marketing consent revision is forbidden without marketing consent"
            }
        })
    }
}

impl std::error::Error for ValidationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DerivationKeyError;

impl fmt::Display for DerivationKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("derivation key must contain at least 32 bytes")
    }
}

impl std::error::Error for DerivationKeyError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreInterestRegistrationResponse {
    pub receipt_id: Uuid,
    pub status: &'static str,
    pub accepted_at: String,
    pub next_step_url: &'static str,
}

pub fn validate_registration(
    request: PreInterestRegistrationRequest,
    verified_host: &str,
) -> Result<ValidatedPreInterestRegistration, ValidationError> {
    let PreInterestRegistrationRequest {
        request_id,
        email,
        party_type,
        organization_name,
        interest_areas,
        consent_revision,
        consented_at,
        source_host: request_source_host,
        locale,
        referral_code,
        display_name,
        website_url,
        registration_consent,
        marketing_consent,
        marketing_consent_revision,
    } = request;

    let source_host = RegistrationHost::parse(verified_host)?;
    if request_source_host != verified_host {
        return Err(ValidationError::SourceHostMismatch);
    }
    if party_type != source_host.party_type() {
        return Err(ValidationError::PartyHostMismatch);
    }
    if !registration_consent {
        return Err(ValidationError::RegistrationConsentRequired);
    }

    let email = SensitiveText(normalize_email(&email)?);
    let organization_name = match party_type {
        PartyType::Individual => {
            if organization_name.is_some() {
                return Err(ValidationError::OrganizationNameForbidden);
            }
            None
        }
        PartyType::Organization => {
            let value = organization_name
                .as_deref()
                .ok_or(ValidationError::OrganizationNameRequired)?;
            Some(SensitiveText(normalize_human_text(
                value,
                MAX_ORGANIZATION_CHARS,
                ValidationError::InvalidOrganizationName,
            )?))
        }
    };
    let display_name = display_name
        .as_deref()
        .map(|value| {
            normalize_human_text(
                value,
                MAX_DISPLAY_NAME_CHARS,
                ValidationError::InvalidDisplayName,
            )
        })
        .transpose()?
        .map(SensitiveText);
    let website_url = website_url
        .as_deref()
        .map(normalize_website_url)
        .transpose()?
        .map(SensitiveText);

    if interest_areas.is_empty() {
        return Err(ValidationError::EmptyInterestSet);
    }
    if interest_areas.len() > MAX_INTERESTS {
        return Err(ValidationError::TooManyInterests);
    }
    let expected_interest_count = interest_areas.len();
    let interest_areas: BTreeSet<_> = interest_areas.into_iter().collect();
    if interest_areas.len() != expected_interest_count {
        return Err(ValidationError::DuplicateInterest);
    }

    let consent_revision = consent_revision.trim().to_owned();
    if !is_portable_identifier(&consent_revision, MAX_CONSENT_REVISION_BYTES) {
        return Err(ValidationError::InvalidConsentRevision);
    }
    let consented_at = consented_at.trim().to_owned();
    if !is_rfc3339(&consented_at) {
        return Err(ValidationError::InvalidConsentedAt);
    }

    let locale = locale.as_deref().map(normalize_locale).transpose()?;
    let referral_code = referral_code
        .as_deref()
        .map(normalize_referral)
        .transpose()?
        .map(SensitiveText);
    let marketing_consent_revision =
        match (marketing_consent, marketing_consent_revision.as_deref()) {
            (true, Some(value)) => {
                let value = value.trim();
                if !is_portable_identifier(value, MAX_MARKETING_CONSENT_REVISION_BYTES) {
                    return Err(ValidationError::InvalidMarketingConsentRevision);
                }
                Some(value.to_owned())
            }
            (true, None) => {
                return Err(ValidationError::InvalidMarketingConsentRevision);
            }
            (false, Some(_)) => {
                return Err(ValidationError::UnexpectedMarketingConsentRevision);
            }
            (false, None) => None,
        };

    Ok(ValidatedPreInterestRegistration {
        request_id,
        email,
        party_type,
        organization_name,
        interest_areas: interest_areas.into_iter().collect(),
        consent_revision,
        consented_at,
        source_host,
        locale,
        referral_code,
        display_name,
        website_url,
        registration_consent: true,
        marketing_consent,
        marketing_consent_revision,
    })
}

pub fn derive_email_alias(
    key: &[u8],
    email: &SensitiveText,
) -> Result<OpaqueDigest, DerivationKeyError> {
    let mut mac = new_mac(key, EMAIL_ALIAS_CONTEXT)?;
    update_field(&mut mac, email.expose().as_bytes());
    Ok(finish_mac(mac))
}

pub fn derive_request_fingerprint(
    key: &[u8],
    registration: &ValidatedPreInterestRegistration,
) -> Result<OpaqueDigest, DerivationKeyError> {
    let mut mac = new_mac(key, REQUEST_FINGERPRINT_CONTEXT)?;
    update_field(&mut mac, registration.email.expose().as_bytes());
    update_field(
        &mut mac,
        party_type_name(registration.party_type).as_bytes(),
    );
    update_optional(
        &mut mac,
        registration
            .organization_name
            .as_ref()
            .map(SensitiveText::expose),
    );
    for interest in &registration.interest_areas {
        update_field(&mut mac, interest.as_str().as_bytes());
    }
    update_field(&mut mac, registration.consent_revision.as_bytes());
    update_field(&mut mac, registration.consented_at.as_bytes());
    update_field(&mut mac, registration.source_host.as_str().as_bytes());
    update_optional(&mut mac, registration.locale.as_deref());
    update_optional(
        &mut mac,
        registration
            .referral_code
            .as_ref()
            .map(SensitiveText::expose),
    );
    update_optional(
        &mut mac,
        registration
            .display_name
            .as_ref()
            .map(SensitiveText::expose),
    );
    update_optional(
        &mut mac,
        registration
            .website_url
            .as_ref()
            .map(SensitiveText::expose),
    );
    update_field(
        &mut mac,
        boolean_name(registration.registration_consent).as_bytes(),
    );
    update_field(
        &mut mac,
        boolean_name(registration.marketing_consent).as_bytes(),
    );
    update_optional(
        &mut mac,
        registration.marketing_consent_revision.as_deref(),
    );
    Ok(finish_mac(mac))
}

pub fn accepted_response(
    receipt_id: Uuid,
    accepted_at: impl Into<String>,
    _party_type: PartyType,
) -> Result<PreInterestRegistrationResponse, ValidationError> {
    let accepted_at = accepted_at.into();
    if !is_rfc3339(&accepted_at) {
        return Err(ValidationError::InvalidConsentedAt);
    }
    Ok(PreInterestRegistrationResponse {
        receipt_id,
        status: "accepted",
        accepted_at,
        next_step_url: QUOTE_NEXT_STEP_URL,
    })
}

fn new_mac(key: &[u8], context: &[u8]) -> Result<HmacSha256, DerivationKeyError> {
    if key.len() < MIN_DERIVATION_KEY_BYTES {
        return Err(DerivationKeyError);
    }
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| DerivationKeyError)?;
    mac.update(context);
    Ok(mac)
}

fn update_field(mac: &mut HmacSha256, value: &[u8]) {
    mac.update(value);
    mac.update(&[0]);
}

fn update_optional(mac: &mut HmacSha256, value: Option<&str>) {
    match value {
        Some(value) => {
            mac.update(&[1]);
            update_field(mac, value.as_bytes());
        }
        None => mac.update(&[0, 0]),
    }
}

fn finish_mac(mac: HmacSha256) -> OpaqueDigest {
    let bytes = mac.finalize().into_bytes();
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&bytes);
    OpaqueDigest(digest)
}

fn normalize_email(value: &str) -> Result<String, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_EMAIL_BYTES
        || !trimmed.is_ascii()
        || trimmed
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ValidationError::InvalidEmail);
    }

    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err(ValidationError::InvalidEmail);
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !domain.contains('.')
        || !domain.split('.').all(valid_domain_label)
    {
        return Err(ValidationError::InvalidEmail);
    }

    Ok(format!(
        "{}@{}",
        local.to_ascii_lowercase(),
        domain.to_ascii_lowercase()
    ))
}

fn valid_domain_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn normalize_human_text(
    value: &str,
    max_chars: usize,
    error: ValidationError,
) -> Result<String, ValidationError> {
    if value.chars().any(char::is_control) {
        return Err(error);
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.chars().count() > max_chars {
        return Err(error);
    }
    Ok(normalized)
}

fn normalize_website_url(value: &str) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.len() > MAX_WEBSITE_URL_BYTES
        || normalized.chars().any(char::is_control)
        || normalized.chars().any(char::is_whitespace)
    {
        return Err(ValidationError::InvalidWebsiteUrl);
    }
    let parsed = Url::parse(normalized).map_err(|_| ValidationError::InvalidWebsiteUrl)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ValidationError::InvalidWebsiteUrl);
    }
    Ok(parsed.to_string())
}

fn normalize_locale(value: &str) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if normalized.len() < 2 || normalized.len() > MAX_LOCALE_BYTES || !is_locale(normalized) {
        return Err(ValidationError::InvalidLocale);
    }
    Ok(normalized.to_owned())
}

fn normalize_referral(value: &str) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if !is_portable_identifier(normalized, MAX_REFERRAL_BYTES) {
        return Err(ValidationError::InvalidReferralCode);
    }
    Ok(normalized.to_owned())
}

fn is_portable_identifier(value: &str, max_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max_bytes
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

fn is_locale(value: &str) -> bool {
    let mut segments = value.split('-');
    let Some(language) = segments.next() else {
        return false;
    };
    if !(2..=3).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }
    segments.all(|segment| {
        (2..=8).contains(&segment.len()) && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !(20..=35).contains(&bytes.len())
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || !bytes[..4].iter().all(u8::is_ascii_digit)
    {
        return false;
    }

    let (Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse_two(bytes, 5),
        parse_two(bytes, 8),
        parse_two(bytes, 11),
        parse_two(bytes, 14),
        parse_two(bytes, 17),
    ) else {
        return false;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }

    let mut timezone_index = 19;
    if bytes.get(timezone_index) == Some(&b'.') {
        timezone_index += 1;
        let fraction_start = timezone_index;
        while bytes.get(timezone_index).is_some_and(u8::is_ascii_digit) {
            timezone_index += 1;
        }
        if timezone_index == fraction_start {
            return false;
        }
    }

    match bytes.get(timezone_index) {
        Some(b'Z') => timezone_index + 1 == bytes.len(),
        Some(b'+') | Some(b'-') => {
            timezone_index + 6 == bytes.len()
                && bytes.get(timezone_index + 3) == Some(&b':')
                && parse_two(bytes, timezone_index + 1).is_some_and(|hour| hour <= 23)
                && parse_two(bytes, timezone_index + 4).is_some_and(|minute| minute <= 59)
        }
        _ => false,
    }
}

fn parse_two(bytes: &[u8], start: usize) -> Option<u8> {
    let first = *bytes.get(start)?;
    let second = *bytes.get(start + 1)?;
    if !first.is_ascii_digit() || !second.is_ascii_digit() {
        return None;
    }
    Some((first - b'0') * 10 + (second - b'0'))
}

const fn party_type_name(party_type: PartyType) -> &'static str {
    match party_type {
        PartyType::Individual => "individual",
        PartyType::Organization => "organization",
    }
}

const fn boolean_name(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &[u8] = b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn individual_request() -> PreInterestRegistrationRequest {
        PreInterestRegistrationRequest {
            request_id: Uuid::parse_str("6ba7b810-9dad-41d1-80b4-00c04fd430c8")
                .expect("fixture UUID"),
            email: " Person@EXAMPLE.COM ".into(),
            party_type: PartyType::Individual,
            organization_name: None,
            interest_areas: vec![InterestArea::Soc2, InterestArea::ReadinessAssessment],
            consent_revision: "pre-interest-v1".into(),
            consented_at: "2026-09-01T15:30:00Z".into(),
            source_host: "user.canonical.plus".into(),
            locale: Some("en-US".into()),
            referral_code: Some("launch-2026".into()),
            display_name: Some("  Person   Name  ".into()),
            website_url: Some("https://example.com/security?source=interest".into()),
            registration_consent: true,
            marketing_consent: false,
            marketing_consent_revision: None,
        }
    }

    #[test]
    fn validates_and_canonicalizes_an_individual_registration() {
        let registration = validate_registration(individual_request(), "user.canonical.plus")
            .expect("valid individual");
        assert_eq!(
            registration.normalized_email().expose(),
            "person@example.com"
        );
        assert_eq!(registration.source_host(), RegistrationHost::User);
        assert_eq!(registration.party_type(), PartyType::Individual);
        assert_eq!(
            registration.interest_areas(),
            &[InterestArea::ReadinessAssessment, InterestArea::Soc2]
        );
        assert_eq!(registration.locale(), Some("en-US"));
        assert_eq!(
            registration.referral_code().map(SensitiveText::expose),
            Some("launch-2026")
        );
        assert_eq!(
            registration.display_name().map(SensitiveText::expose),
            Some("Person Name")
        );
        assert_eq!(
            registration.website_url().map(SensitiveText::expose),
            Some("https://example.com/security?source=interest")
        );
        assert!(registration.organization_name().is_none());
        assert!(registration.registration_consent());
        assert!(!registration.marketing_consent());
        assert!(registration.marketing_consent_revision().is_none());
    }

    #[test]
    fn organization_requires_a_name_and_correct_host() {
        let mut request = individual_request();
        request.party_type = PartyType::Organization;
        request.source_host = "org.canonical.plus".into();
        assert_eq!(
            validate_registration(request.clone(), "org.canonical.plus"),
            Err(ValidationError::OrganizationNameRequired)
        );

        request.organization_name = Some("  Example   Security  ".into());
        let registration = validate_registration(request, "org.canonical.plus").expect("valid org");
        assert_eq!(
            registration.organization_name().map(SensitiveText::expose),
            Some("Example Security")
        );
    }

    #[test]
    fn request_and_verified_hosts_must_match() {
        assert_eq!(
            validate_registration(individual_request(), "org.canonical.plus"),
            Err(ValidationError::SourceHostMismatch)
        );

        let mut request = individual_request();
        request.party_type = PartyType::Organization;
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::PartyHostMismatch)
        );
    }

    #[test]
    fn registration_and_marketing_consent_are_independent_and_explicit() {
        let mut request = individual_request();
        request.registration_consent = false;
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::RegistrationConsentRequired)
        );

        let mut request = individual_request();
        request.marketing_consent = true;
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::InvalidMarketingConsentRevision)
        );

        let mut request = individual_request();
        request.marketing_consent_revision = Some("marketing-v1".into());
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::UnexpectedMarketingConsentRevision)
        );

        let mut request = individual_request();
        request.marketing_consent = true;
        request.marketing_consent_revision = Some("marketing-v1".into());
        let registration = validate_registration(request, "user.canonical.plus")
            .expect("valid explicit marketing consent");
        assert!(registration.marketing_consent());
        assert_eq!(
            registration.marketing_consent_revision(),
            Some("marketing-v1")
        );
    }

    #[test]
    fn rejects_duplicate_interests_and_unsafe_metadata() {
        let mut request = individual_request();
        request.interest_areas.push(InterestArea::Soc2);
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::DuplicateInterest)
        );

        let mut request = individual_request();
        request.referral_code = Some("https://tracker.example".into());
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::InvalidReferralCode)
        );

        let mut request = individual_request();
        request.website_url = Some("http://example.com".into());
        assert_eq!(
            validate_registration(request, "user.canonical.plus"),
            Err(ValidationError::InvalidWebsiteUrl)
        );
    }

    #[test]
    fn deserialization_rejects_unknown_and_missing_required_fields() {
        let mut value = serde_json::to_value(individual_request()).expect("serialize");
        value["notes"] = serde_json::json!("sensitive free-form text");
        assert!(serde_json::from_value::<PreInterestRegistrationRequest>(value).is_err());

        let mut value = serde_json::to_value(individual_request()).expect("serialize");
        value["requestId"] = serde_json::json!("person@example.com");
        assert!(serde_json::from_value::<PreInterestRegistrationRequest>(value).is_err());

        let mut value = serde_json::to_value(individual_request()).expect("serialize");
        value
            .as_object_mut()
            .expect("request object")
            .remove("registrationConsent");
        assert!(serde_json::from_value::<PreInterestRegistrationRequest>(value).is_err());
    }

    #[test]
    fn debug_output_redacts_contact_fields() {
        let registration = validate_registration(individual_request(), "user.canonical.plus")
            .expect("valid request");
        let debug = format!("{registration:?}");
        assert!(!debug.contains("person@example.com"));
        assert!(!debug.contains("launch-2026"));
        assert!(!debug.contains("Person Name"));
        assert!(!debug.contains("example.com/security"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn aliases_and_fingerprints_are_keyed_and_domain_separated() {
        let registration = validate_registration(individual_request(), "user.canonical.plus")
            .expect("valid request");
        let alias_a = derive_email_alias(KEY_A, registration.normalized_email()).expect("alias");
        let alias_b = derive_email_alias(KEY_B, registration.normalized_email()).expect("alias");
        let fingerprint = derive_request_fingerprint(KEY_A, &registration).expect("fingerprint");

        assert_eq!(
            alias_a,
            derive_email_alias(KEY_A, registration.normalized_email()).expect("alias")
        );
        assert_ne!(alias_a, alias_b);
        assert_ne!(alias_a, fingerprint);
        assert_eq!(alias_a.as_bytes().len(), 32);
        assert_eq!(alias_a.to_hex().len(), 64);
        assert_eq!(
            derive_email_alias(b"short", registration.normalized_email()),
            Err(DerivationKeyError)
        );
    }

    #[test]
    fn request_fingerprint_covers_new_consent_and_contact_fields() {
        let first = validate_registration(individual_request(), "user.canonical.plus")
            .expect("valid request");
        let mut reversed = individual_request();
        reversed.interest_areas.reverse();
        let second = validate_registration(reversed, "user.canonical.plus").expect("valid request");
        assert_eq!(
            derive_request_fingerprint(KEY_A, &first).expect("fingerprint"),
            derive_request_fingerprint(KEY_A, &second).expect("fingerprint")
        );

        let mut changed = individual_request();
        changed.display_name = Some("Different Person".into());
        let changed = validate_registration(changed, "user.canonical.plus").expect("valid request");
        assert_ne!(
            derive_request_fingerprint(KEY_A, &first).expect("fingerprint"),
            derive_request_fingerprint(KEY_A, &changed).expect("fingerprint")
        );

        let mut changed = individual_request();
        changed.marketing_consent = true;
        changed.marketing_consent_revision = Some("marketing-v1".into());
        let changed = validate_registration(changed, "user.canonical.plus").expect("valid request");
        assert_ne!(
            derive_request_fingerprint(KEY_A, &first).expect("fingerprint"),
            derive_request_fingerprint(KEY_A, &changed).expect("fingerprint")
        );
    }

    #[test]
    fn accepted_response_exposes_no_new_or_existing_state() {
        let response = accepted_response(
            Uuid::parse_str("ca761232-ed42-11ce-bacd-00aa0057b223").expect("fixture UUID"),
            "2026-09-01T15:32:00Z",
            PartyType::Individual,
        )
        .expect("response");
        let json = serde_json::to_value(response).expect("serialize response");
        assert_eq!(json["status"], "accepted");
        assert_eq!(json["nextStepUrl"], "https://user.canonical.plus/u/quote");
        assert!(json.get("created").is_none());
        assert!(json.get("duplicate").is_none());
        assert!(json.get("email").is_none());
    }

    #[test]
    fn sql_candidate_contains_privacy_idempotency_consent_and_outbox_boundaries() {
        let sql = include_str!("../db/candidates/pre_interest_v1.sql");
        for required in [
            "CREATE SCHEMA canonical_cloud__interest;",
            "email_alias bytea NOT NULL",
            "request_fingerprint bytea NOT NULL",
            "display_name_ciphertext bytea",
            "website_url_ciphertext bytea",
            "registration_consent boolean NOT NULL",
            "marketing_consent boolean NOT NULL",
            "marketing_consent_revision text",
            "UNIQUE (request_id)",
            "canonical_pre_interest_consent_event",
            "canonical_pre_interest_outbox",
            "FORCE ROW LEVEL SECURITY",
            "octet_length(email_alias) = 32",
            "octet_length(request_fingerprint) = 32",
        ] {
            assert!(sql.contains(required), "missing SQL boundary: {required}");
        }
        for forbidden in [
            "CREATE ROLE",
            "GRANT ALL",
            "IF NOT EXISTS",
            "BEGIN;",
            "COMMIT;",
        ] {
            assert!(
                !sql.contains(forbidden),
                "forbidden lifecycle SQL: {forbidden}"
            );
        }
    }
}
