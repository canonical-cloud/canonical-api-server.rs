use std::collections::BTreeSet;
use std::fmt;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const MAX_EMAIL_BYTES: usize = 320;
const MAX_ORGANIZATION_CHARS: usize = 200;
const MAX_INTERESTS: usize = 9;
const MAX_CONSENT_REVISION_BYTES: usize = 64;
const MAX_LOCALE_BYTES: usize = 35;
const MAX_REFERRAL_BYTES: usize = 64;
const MIN_DERIVATION_KEY_BYTES: usize = 32;
const EMAIL_ALIAS_CONTEXT: &[u8] = b"canonical.pre-interest.email-alias.v1\0";
const REQUEST_FINGERPRINT_CONTEXT: &[u8] =
    b"canonical.pre-interest.request-fingerprint.v1\0";

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PartyType {
    Individual,
    Organization,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
}

impl ValidatedPreInterestRegistration {
    #[must_use]
    pub const fn request_id(&self) -> Uuid {
        self.request_id
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
    InvalidEmail,
    InvalidLocale,
    InvalidOrganizationName,
    InvalidReferralCode,
    InvalidSourceHost,
    OrganizationNameForbidden,
    OrganizationNameRequired,
    PartyHostMismatch,
    SourceHostMismatch,
    TooManyInterests,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateInterest => "interest areas must be unique",
            Self::EmptyInterestSet => "at least one interest area is required",
            Self::InvalidConsentRevision => "consent revision is invalid",
            Self::InvalidConsentedAt => "consent timestamp is invalid",
            Self::InvalidEmail => "email is invalid",
            Self::InvalidLocale => "locale is invalid",
            Self::InvalidOrganizationName => "organization name is invalid",
            Self::InvalidReferralCode => "referral code is invalid",
            Self::InvalidSourceHost => "registration host is invalid",
            Self::OrganizationNameForbidden => {
                "organization name is forbidden for individual registration"
            }
            Self::OrganizationNameRequired => {
                "organization name is required for organization registration"
            }
            Self::PartyHostMismatch => "registration host and party type do not agree",
            Self::SourceHostMismatch => {
                "request source host does not match the verified request host"
            }
            Self::TooManyInterests => "too many interest areas were supplied",
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
    let source_host = RegistrationHost::parse(verified_host)?;
    if request.source_host != verified_host {
        return Err(ValidationError::SourceHostMismatch);
    }
    if request.party_type != source_host.party_type() {
        return Err(ValidationError::PartyHostMismatch);
    }

    let email = SensitiveText(normalize_email(&request.email)?);
    let organization_name = match request.party_type {
        PartyType::Individual => {
            if request.organization_name.is_some() {
                return Err(ValidationError::OrganizationNameForbidden);
            }
            None
        }
        PartyType::Organization => {
            let value = request
                .organization_name
                .as_deref()
                .ok_or(ValidationError::OrganizationNameRequired)?;
            Some(SensitiveText(normalize_organization_name(value)?))
        }
    };

    if request.interest_areas.is_empty() {
        return Err(ValidationError::EmptyInterestSet);
    }
    if request.interest_areas.len() > MAX_INTERESTS {
        return Err(ValidationError::TooManyInterests);
    }
    let mut unique_interests = BTreeSet::new();
    for interest in request.interest_areas {
        if !unique_interests.insert(interest) {
            return Err(ValidationError::DuplicateInterest);
        }
    }

    let consent_revision = request.consent_revision.trim().to_owned();
    if !is_portable_identifier(&consent_revision, MAX_CONSENT_REVISION_BYTES) {
        return Err(ValidationError::InvalidConsentRevision);
    }
    let consented_at = request.consented_at.trim().to_owned();
    if !is_rfc3339(&consented_at) {
        return Err(ValidationError::InvalidConsentedAt);
    }

    let locale = request
        .locale
        .as_deref()
        .map(normalize_locale)
        .transpose()?;
    let referral_code = request
        .referral_code
        .as_deref()
        .map(normalize_referral)
        .transpose()?
        .map(SensitiveText);

    Ok(ValidatedPreInterestRegistration {
        request_id: request.request_id,
        email,
        party_type: request.party_type,
        organization_name,
        interest_areas: unique_interests.into_iter().collect(),
        consent_revision,
        consented_at,
        source_host,
        locale,
        referral_code,
    })
}

pub fn derive_email_alias(
    key: &[u8],
    email: &SensitiveText,
) -> Result<OpaqueDigest, DerivationKeyError> {
    derive_hmac(key, EMAIL_ALIAS_CONTEXT, &[email.expose().as_bytes()])
}

pub fn derive_request_fingerprint(
    key: &[u8],
    registration: &ValidatedPreInterestRegistration,
) -> Result<OpaqueDigest, DerivationKeyError> {
    let mut fields = vec![
        registration.email.expose().as_bytes(),
        party_type_name(registration.party_type).as_bytes(),
        registration.consent_revision.as_bytes(),
        registration.consented_at.as_bytes(),
        registration.source_host.as_str().as_bytes(),
        registration.locale.as_deref().unwrap_or_default().as_bytes(),
        registration
            .organization_name
            .as_ref()
            .map_or(&[][..], |value| value.expose().as_bytes()),
        registration
            .referral_code
            .as_ref()
            .map_or(&[][..], |value| value.expose().as_bytes()),
    ];
    let interest_names: Vec<&[u8]> = registration
        .interest_areas
        .iter()
        .map(|interest| interest.as_str().as_bytes())
        .collect();
    fields.extend(interest_names);
    derive_hmac(key, REQUEST_FINGERPRINT_CONTEXT, &fields)
}

pub fn accepted_response(
    receipt_id: Uuid,
    accepted_at: impl Into<String>,
    party_type: PartyType,
) -> Result<PreInterestRegistrationResponse, ValidationError> {
    let accepted_at = accepted_at.into();
    if !is_rfc3339(&accepted_at) {
        return Err(ValidationError::InvalidConsentedAt);
    }
    let next_step_url = match party_type {
        PartyType::Individual => "https://user.canonical.plus/u/readiness",
        PartyType::Organization => "https://org.canonical.plus/o/readiness",
    };
    Ok(PreInterestRegistrationResponse {
        receipt_id,
        status: "accepted",
        accepted_at,
        next_step_url,
    })
}

fn derive_hmac(
    key: &[u8],
    context: &[u8],
    fields: &[&[u8]],
) -> Result<OpaqueDigest, DerivationKeyError> {
    if key.len() < MIN_DERIVATION_KEY_BYTES {
        return Err(DerivationKeyError);
    }
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| DerivationKeyError)?;
    mac.update(context);
    for field in fields {
        let length = u64::try_from(field.len()).unwrap_or(u64::MAX);
        mac.update(&length.to_be_bytes());
        mac.update(field);
    }
    let bytes = mac.finalize().into_bytes();
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&bytes);
    Ok(OpaqueDigest(digest))
}

fn normalize_email(value: &str) -> Result<String, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_EMAIL_BYTES
        || trimmed
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ValidationError::InvalidEmail);
    }

    let mut parts = trimmed.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || local.is_empty()
        || domain.is_empty()
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !domain.contains('.')
    {
        return Err(ValidationError::InvalidEmail);
    }
    if !domain.split('.').all(valid_domain_label) {
        return Err(ValidationError::InvalidEmail);
    }

    let normalized = format!("{}@{}", local.to_lowercase(), domain.to_ascii_lowercase());
    if normalized.len() > MAX_EMAIL_BYTES {
        return Err(ValidationError::InvalidEmail);
    }
    Ok(normalized)
}

fn valid_domain_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn normalize_organization_name(value: &str) -> Result<String, ValidationError> {
    if value.chars().any(char::is_control) {
        return Err(ValidationError::InvalidOrganizationName);
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.chars().count() > MAX_ORGANIZATION_CHARS {
        return Err(ValidationError::InvalidOrganizationName);
    }
    Ok(normalized)
}

fn normalize_locale(value: &str) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if normalized.len() < 2
        || normalized.len() > MAX_LOCALE_BYTES
        || !is_locale(normalized)
    {
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
        (2..=8).contains(&segment.len())
            && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
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
    {
        return false;
    }

    let Some(month) = parse_two(bytes, 5) else {
        return false;
    };
    let Some(day) = parse_two(bytes, 8) else {
        return false;
    };
    let Some(hour) = parse_two(bytes, 11) else {
        return false;
    };
    let Some(minute) = parse_two(bytes, 14) else {
        return false;
    };
    let Some(second) = parse_two(bytes, 17) else {
        return false;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !bytes[..4].iter().all(u8::is_ascii_digit)
    {
        return false;
    }

    let mut timezone_index = 19;
    if bytes.get(timezone_index) == Some(&b'.') {
        timezone_index += 1;
        let fraction_start = timezone_index;
        while bytes
            .get(timezone_index)
            .is_some_and(u8::is_ascii_digit)
        {
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
            interest_areas: vec![
                InterestArea::Soc2,
                InterestArea::ReadinessAssessment,
            ],
            consent_revision: "pre-interest-v1".into(),
            consented_at: "2026-09-01T15:30:00Z".into(),
            source_host: "user.canonical.plus".into(),
            locale: Some("en-US".into()),
            referral_code: Some("launch-2026".into()),
        }
    }

    #[test]
    fn validates_and_canonicalizes_an_individual_registration() {
        let registration =
            validate_registration(individual_request(), "user.canonical.plus")
                .expect("valid individual");
        assert_eq!(registration.normalized_email().expose(), "person@example.com");
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
        assert!(registration.organization_name().is_none());
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
        let registration =
            validate_registration(request, "org.canonical.plus").expect("valid org");
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
    }

    #[test]
    fn deserialization_rejects_unknown_and_malformed_fields() {
        let mut value = serde_json::to_value(individual_request()).expect("serialize");
        value["notes"] = serde_json::json!("sensitive free-form text");
        assert!(serde_json::from_value::<PreInterestRegistrationRequest>(value).is_err());

        let mut value = serde_json::to_value(individual_request()).expect("serialize");
        value["requestId"] = serde_json::json!("person@example.com");
        assert!(serde_json::from_value::<PreInterestRegistrationRequest>(value).is_err());
    }

    #[test]
    fn debug_output_redacts_contact_fields() {
        let registration =
            validate_registration(individual_request(), "user.canonical.plus")
                .expect("valid request");
        let debug = format!("{registration:?}");
        assert!(!debug.contains("person@example.com"));
        assert!(!debug.contains("launch-2026"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn alias_and_request_fingerprint_are_keyed_and_domain_separated() {
        let registration =
            validate_registration(individual_request(), "user.canonical.plus")
                .expect("valid request");
        let alias_a =
            derive_email_alias(KEY_A, registration.normalized_email()).expect("alias");
        let alias_b =
            derive_email_alias(KEY_B, registration.normalized_email()).expect("alias");
        let fingerprint =
            derive_request_fingerprint(KEY_A, &registration).expect("fingerprint");

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
    fn request_fingerprint_is_independent_of_interest_input_order() {
        let first =
            validate_registration(individual_request(), "user.canonical.plus")
                .expect("valid request");
        let mut reversed = individual_request();
        reversed.interest_areas.reverse();
        let second =
            validate_registration(reversed, "user.canonical.plus").expect("valid request");
        assert_eq!(
            derive_request_fingerprint(KEY_A, &first).expect("fingerprint"),
            derive_request_fingerprint(KEY_A, &second).expect("fingerprint")
        );

        let mut changed = individual_request();
        changed.consent_revision = "pre-interest-v2".into();
        let changed =
            validate_registration(changed, "user.canonical.plus").expect("valid request");
        assert_ne!(
            derive_request_fingerprint(KEY_A, &first).expect("fingerprint"),
            derive_request_fingerprint(KEY_A, &changed).expect("fingerprint")
        );
    }

    #[test]
    fn accepted_response_exposes_no_new_or_existing_state() {
        let response = accepted_response(
            Uuid::parse_str("ca761232-ed42-11ce-bacd-00aa0057b223")
                .expect("fixture UUID"),
            "2026-09-01T15:32:00Z",
            PartyType::Individual,
        )
        .expect("response");
        let json = serde_json::to_value(response).expect("serialize response");
        assert_eq!(json["status"], "accepted");
        assert_eq!(
            json["nextStepUrl"],
            "https://user.canonical.plus/u/readiness"
        );
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
            "UNIQUE (request_id)",
            "canonical_pre_interest_consent_event",
            "canonical_pre_interest_outbox",
            "FORCE ROW LEVEL SECURITY",
            "octet_length(email_alias) = 32",
            "octet_length(request_fingerprint) = 32",
        ] {
            assert!(sql.contains(required), "missing SQL boundary: {required}");
        }
        for forbidden in ["CREATE ROLE", "GRANT ALL", "IF NOT EXISTS", "BEGIN;", "COMMIT;"] {
            assert!(!sql.contains(forbidden), "forbidden lifecycle SQL: {forbidden}");
        }
    }
}
