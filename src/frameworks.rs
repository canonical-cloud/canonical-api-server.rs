use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::ApiError;

pub(crate) const READINESS_BOUNDARY: &str = "Readiness reference data. Canonical Plus prepares \
     organizations for independent review; it does not perform audits, attestations, or \
     certifications.";

const MAX_ASSESSMENT_ORGANIZATION_BYTES: usize = 200;
const MAX_ASSESSMENT_NOTES_BYTES: usize = 2_000;

const ASSESSMENT_FIELDS: &[&str] = &["frameworkIds", "notes", "organization"];

/// One entry in the static readiness reference catalog. The catalog describes
/// frameworks Canonical Plus prepares organizations for; independent auditors,
/// assessors, and certification bodies perform the actual audits and
/// attestations.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrameworkDescriptor {
    pub category: &'static str,
    pub governing_body: &'static str,
    pub id: &'static str,
    pub source_url: &'static str,
    pub summary: &'static str,
    pub title: &'static str,
}

pub(crate) const CATALOG: &[FrameworkDescriptor] = &[
    FrameworkDescriptor {
        category: "security-program",
        governing_body: "Center for Internet Security",
        id: "cis-controls-8.1",
        source_url: "https://www.cisecurity.org/controls/v8-1",
        summary: "Prioritized safeguards for building and benchmarking a security program. \
             Canonical Plus maps readiness work to the safeguards ahead of any independent \
             assessment.",
        title: "CIS Critical Security Controls",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "U.S. Department of Defense",
        id: "cmmc-2.0",
        source_url: "https://dodcio.defense.gov/CMMC/",
        summary: "Defense industrial base maturity model assessed by authorized C3PAOs. \
             Canonical Plus prepares the CUI boundary and evidence for that independent \
             assessment.",
        title: "Cybersecurity Maturity Model Certification",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "Cloud Security Alliance",
        id: "csa-ccm",
        source_url: "https://cloudsecurityalliance.org/research/cloud-controls-matrix",
        summary: "Cloud control matrix behind the STAR assurance program. Canonical Plus \
             readies cloud control evidence for independent STAR review.",
        title: "CSA Cloud Controls Matrix and STAR",
    },
    FrameworkDescriptor {
        category: "security-program",
        governing_body: "European Union",
        id: "dora-2022-2554",
        source_url: "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
        summary: "EU operational-resilience regulation for financial entities. Canonical Plus \
             prepares ICT risk, testing, and reporting practices for supervisory scrutiny.",
        title: "Digital Operational Resilience Act",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "U.S. General Services Administration",
        id: "fedramp-rev5",
        source_url: "https://www.fedramp.gov/documents-templates/",
        summary: "U.S. federal cloud authorization program assessed by independent 3PAOs. \
             Canonical Plus prepares the authorization boundary and package for that \
             assessment.",
        title: "Federal Risk and Authorization Management Program",
    },
    FrameworkDescriptor {
        category: "privacy-regulation",
        governing_body: "European Union",
        id: "gdpr-2016-679",
        source_url: "https://eur-lex.europa.eu/eli/reg/2016/679/oj",
        summary: "EU data-protection regulation enforced by supervisory authorities. \
             Canonical Plus prepares processing records, lawful bases, and safeguards for \
             regulator review.",
        title: "General Data Protection Regulation",
    },
    FrameworkDescriptor {
        category: "privacy-regulation",
        governing_body: "U.S. Department of Health and Human Services",
        id: "hipaa-security-rule",
        source_url: "https://www.hhs.gov/hipaa/for-professionals/security/index.html",
        summary: "U.S. health-information security regulation overseen by HHS OCR. Canonical \
             Plus prepares the required safeguards and documentation for independent review.",
        title: "HIPAA Security Rule",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "ISO",
        id: "iso-22301-2019",
        source_url: "https://www.iso.org/standard/75106.html",
        summary: "Business-continuity management standard certified by accredited bodies. \
             Canonical Plus prepares the continuity program for that independent \
             certification audit.",
        title: "ISO 22301 Business Continuity Management",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "ISO/IEC",
        id: "iso-iec-27001-2022",
        source_url: "https://www.iso.org/standard/27001",
        summary: "Information-security management system standard certified by accredited \
             bodies. Canonical Plus prepares the ISMS and evidence for that independent \
             certification audit.",
        title: "ISO/IEC 27001",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "ISO/IEC",
        id: "iso-iec-27701",
        source_url: "https://www.iso.org/standard/85819.html",
        summary: "Privacy information management extension to ISO/IEC 27001. Canonical Plus \
             prepares the privacy program for independent certification against it.",
        title: "ISO/IEC 27701 Privacy Information Management",
    },
    FrameworkDescriptor {
        category: "security-program",
        governing_body: "European Union",
        id: "nis2-2022-2555",
        source_url: "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
        summary: "EU cybersecurity directive for essential and important entities. Canonical \
             Plus prepares governance, risk, and reporting practices for national-authority \
             oversight.",
        title: "NIS2 Directive",
    },
    FrameworkDescriptor {
        category: "security-program",
        governing_body: "National Institute of Standards and Technology",
        id: "nist-csf-2.0",
        source_url: "https://www.nist.gov/cyberframework",
        summary: "Voluntary cybersecurity outcomes framework across six functions. Canonical \
             Plus uses it to structure readiness work ahead of any independent evaluation.",
        title: "NIST Cybersecurity Framework",
    },
    FrameworkDescriptor {
        category: "security-program",
        governing_body: "National Institute of Standards and Technology",
        id: "nist-sp-800-53-5.2.0",
        source_url:
            "https://csrc.nist.gov/projects/risk-management/sp800-53-controls/release-search",
        summary: "Federal control catalog underpinning RMF authorizations. Canonical Plus \
             prepares control implementations and evidence for independent assessment.",
        title: "NIST SP 800-53 Control Catalog",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "PCI Security Standards Council",
        id: "pci-dss-4.0.1",
        source_url: "https://www.pcisecuritystandards.org/standards/pci-dss/",
        summary: "Payment-card security standard validated by QSAs or self-assessment. \
             Canonical Plus prepares cardholder-data controls for that independent \
             validation.",
        title: "Payment Card Industry Data Security Standard",
    },
    FrameworkDescriptor {
        category: "assurance-target",
        governing_body: "AICPA",
        id: "soc2-tsc",
        source_url:
            "https://www.aicpa-cima.com/resources/landing/system-and-organization-controls-soc-suite-of-services",
        summary: "Trust Services Criteria examined by independent CPA firms in SOC 2 \
             reports. Canonical Plus prepares controls and evidence for that examination.",
        title: "SOC 2 Trust Services Criteria",
    },
];

pub(crate) fn find(framework_id: &str) -> Option<&'static FrameworkDescriptor> {
    CATALOG
        .iter()
        .find(|descriptor| descriptor.id == framework_id)
}

/// A validated readiness-assessment intake request. Accepting one records
/// preparation scope only; it does not start, promise, or perform an audit.
#[derive(Clone, Debug)]
pub(crate) struct AssessmentSubmission {
    pub framework_ids: Vec<String>,
    pub notes: Option<String>,
    pub organization: String,
}

pub(crate) fn parse_assessment_request(
    payload: JsonValue,
) -> Result<AssessmentSubmission, ApiError> {
    let object = payload.as_object().ok_or_else(|| {
        ApiError::bad_request("invalid_request", "assessment request must be a JSON object")
    })?;
    if object
        .keys()
        .any(|field| !ASSESSMENT_FIELDS.contains(&field.as_str()))
    {
        return Err(ApiError::bad_request(
            "invalid_request",
            "assessment request contains an unknown field",
        ));
    }

    let supplied_ids = object
        .get("frameworkIds")
        .and_then(JsonValue::as_array)
        .filter(|ids| !ids.is_empty() && ids.len() <= CATALOG.len())
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_request",
                "frameworkIds must list 1 to 15 readiness framework ids",
            )
        })?;
    let mut framework_ids = Vec::with_capacity(supplied_ids.len());
    for supplied in supplied_ids {
        let framework_id = supplied
            .as_str()
            .filter(|framework_id| find(framework_id).is_some())
            .ok_or_else(|| {
                ApiError::bad_request(
                    "invalid_request",
                    "frameworkIds may only contain known readiness framework ids",
                )
            })?;
        if framework_ids.iter().any(|existing| existing == framework_id) {
            return Err(ApiError::bad_request(
                "invalid_request",
                "frameworkIds must not contain duplicates",
            ));
        }
        framework_ids.push(framework_id.to_owned());
    }

    let organization = object
        .get("organization")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|organization| {
            !organization.is_empty() && organization.len() <= MAX_ASSESSMENT_ORGANIZATION_BYTES
        })
        .ok_or_else(|| {
            ApiError::bad_request("invalid_request", "organization must contain 1 to 200 bytes")
        })?
        .to_owned();

    let notes = match object.get("notes") {
        None | Some(JsonValue::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|notes| notes.len() <= MAX_ASSESSMENT_NOTES_BYTES)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_request",
                        "notes must be a string of at most 2000 bytes",
                    )
                })?
                .to_owned(),
        ),
    };

    Ok(AssessmentSubmission {
        framework_ids,
        notes,
        organization,
    })
}

#[cfg(test)]
mod tests {
    use super::{find, parse_assessment_request, CATALOG, READINESS_BOUNDARY};
    use serde_json::json;

    #[test]
    fn catalog_contains_fifteen_unique_readiness_frameworks() {
        assert_eq!(CATALOG.len(), 15);
        for descriptor in CATALOG {
            assert!(matches!(
                descriptor.category,
                "assurance-target" | "security-program" | "privacy-regulation"
            ));
            assert!(descriptor.source_url.starts_with("https://"));
            assert_eq!(find(descriptor.id).unwrap().id, descriptor.id);
        }
        let mut ids = CATALOG
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 15);
    }

    #[test]
    fn boundary_never_claims_audit_authority() {
        assert!(READINESS_BOUNDARY.contains("does not perform audits"));
    }

    #[test]
    fn assessment_requests_fail_closed() {
        assert!(parse_assessment_request(json!([])).is_err());
        assert!(parse_assessment_request(json!({
            "frameworkIds": ["soc2-tsc"],
            "organization": "Example Incorporated",
            "unexpected": true
        }))
        .is_err());
        assert!(parse_assessment_request(json!({
            "frameworkIds": ["soc2-tsc", "soc2-tsc"],
            "organization": "Example Incorporated"
        }))
        .is_err());
        assert!(parse_assessment_request(json!({
            "frameworkIds": ["not-a-framework"],
            "organization": "Example Incorporated"
        }))
        .is_err());
        assert!(parse_assessment_request(json!({
            "frameworkIds": ["soc2-tsc"],
            "organization": ""
        }))
        .is_err());
        assert!(parse_assessment_request(json!({
            "frameworkIds": ["soc2-tsc"],
            "organization": "Example Incorporated",
            "notes": "n".repeat(2001)
        }))
        .is_err());

        let accepted = parse_assessment_request(json!({
            "frameworkIds": ["iso-iec-27001-2022", "soc2-tsc"],
            "organization": "  Example Incorporated  ",
            "notes": null
        }))
        .unwrap();
        assert_eq!(accepted.framework_ids.len(), 2);
        assert_eq!(accepted.organization, "Example Incorporated");
        assert!(accepted.notes.is_none());
    }
}
