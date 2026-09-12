use std::collections::HashSet;

use canonical_lib::interfaces::{
    QuoteDetail, QuoteDetailStatus, QuoteEstimate, QuoteListResponse, QuoteProblem, QuoteRequest,
    QuoteRetryResponse, QuoteRetryResponseStatus, QuoteStatusEvent, QuoteStatusEventStage,
    QuoteSubmissionResponse, QuoteSubmissionResponseStatus, QuoteSummary, QuoteSummaryStatus,
};
use serde_json::{Map, Value as JsonValue};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ApiError, QuoteEventRecord, QuoteRecord, APPLICATION_CONTEXT_MARKDOWN,
    MAX_MARKDOWN_CONTEXT_BYTES,
};

const REQUEST_FIELDS: &[&str] = &[
    "organizationName",
    "contactName",
    "contactEmail",
    "website",
    "employeeCount",
    "annualRevenueBand",
    "frameworks",
    "currentStage",
    "infrastructure",
    "dataSensitivity",
    "targetDate",
    "hasSecurityProgram",
    "hasPolicies",
    "hasRiskAssessment",
    "hasIncidentResponsePlan",
    "hasVendorManagement",
    "notes",
    "contextKey",
    "answersVersion",
];

use canonical_lib::quotes::AcceptedQuoteRequest;

pub(crate) fn parse_quote_request(payload: JsonValue) -> Result<AcceptedQuoteRequest, ApiError> {
    let object = payload.as_object().ok_or_else(|| {
        ApiError::bad_request("invalid_request", "quote request must be a JSON object")
    })?;
    let allowed = REQUEST_FIELDS.iter().copied().collect::<HashSet<_>>();
    if object.keys().any(|field| !allowed.contains(field.as_str())) {
        return Err(ApiError::bad_request(
            "invalid_request",
            "quote request contains an unknown field",
        ));
    }

    let mut wire: QuoteRequest = serde_json::from_value(payload).map_err(|_| {
        ApiError::bad_request(
            "invalid_request",
            "quote request does not match the canonical v1 wire contract",
        )
    })?;
    if !canonical_lib::validate_quote_request(&wire).is_accepted() {
        return Err(ApiError::bad_request(
            "invalid_request",
            "quote request failed canonical v1 field validation",
        ));
    }
    if wire.context_key.is_none() {
        wire.context_key = Some("quote-analysis".into());
    }

    let application_markdown = APPLICATION_CONTEXT_MARKDOWN.trim();
    if application_markdown.is_empty() || application_markdown.len() > MAX_MARKDOWN_CONTEXT_BYTES {
        return Err(ApiError::service_unavailable(
            "context_unavailable",
            "quote analysis context is unavailable",
        ));
    }

    Ok(AcceptedQuoteRequest {
        application_markdown: application_markdown.to_owned(),
        wire,
    })
}

pub(crate) fn quote_submission_response(record: &QuoteRecord) -> QuoteSubmissionResponse {
    QuoteSubmissionResponse {
        quote_id: record.quote_id.to_string(),
        status: submission_status(&record.status),
        stream_url: events_url(record.quote_id),
        created_at: record.created_at.clone(),
    }
}

pub(crate) fn quote_retry_response(record: &QuoteRecord) -> QuoteRetryResponse {
    QuoteRetryResponse {
        quote_id: record.quote_id.to_string(),
        status: QuoteRetryResponseStatus::Queued,
        stream_url: events_url(record.quote_id),
        updated_at: record.updated_at.clone(),
    }
}

pub(crate) fn quote_detail(record: &QuoteRecord) -> Result<QuoteDetail, ApiError> {
    Ok(QuoteDetail {
        quote_id: record.quote_id.to_string(),
        status: detail_status(&record.status)?,
        request: record.request.clone(),
        events_url: events_url(record.quote_id),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        estimate: quote_estimate(record)?,
        problem: quote_failure_problem(record),
    })
}

pub(crate) fn quote_list_response(
    mut records: Vec<QuoteRecord>,
    limit: usize,
) -> Result<QuoteListResponse, ApiError> {
    let next_cursor = if records.len() > limit {
        records.truncate(limit);
        records.last().map(|record| record.quote_id.to_string())
    } else {
        None
    };
    let quotes = records
        .iter()
        .map(quote_summary)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(QuoteListResponse {
        quotes,
        next_cursor,
    })
}

pub(crate) fn quote_status_event(
    event: &QuoteEventRecord,
    record: &QuoteRecord,
) -> Result<QuoteStatusEvent, ApiError> {
    let stage = event_stage(&event.status)?;
    let terminal = matches!(
        stage,
        QuoteStatusEventStage::Ready | QuoteStatusEventStage::Failed
    );
    let message = match stage {
        QuoteStatusEventStage::Queued => "Quote accepted and queued for analysis.",
        QuoteStatusEventStage::LoadingContext => "Loading the selected Canonical context.",
        QuoteStatusEventStage::Analyzing => "Analyzing the bounded quote request.",
        QuoteStatusEventStage::Validating => "Validating the bounded estimate response.",
        QuoteStatusEventStage::Ready => "The preliminary quote is ready for review.",
        QuoteStatusEventStage::Failed => "Quote analysis did not complete successfully.",
    };
    Ok(QuoteStatusEvent {
        quote_id: record.quote_id.to_string(),
        sequence: event.sequence.to_string(),
        stage,
        message: message.into(),
        terminal,
        estimate: quote_estimate(record)?,
        occurred_at: event.occurred_at.clone(),
        problem: quote_failure_problem(record),
    })
}

pub(crate) fn now_rfc3339() -> Result<String, ApiError> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| {
        ApiError::service_unavailable(
            "internal",
            "quote timestamp generation is temporarily unavailable",
        )
    })
}

fn events_url(quote_id: Uuid) -> String {
    format!("/api/v1/quotes/{quote_id}/events")
}

fn submission_status(status: &str) -> QuoteSubmissionResponseStatus {
    match status {
        "analyzing" => QuoteSubmissionResponseStatus::Analyzing,
        "completed" | "ready" => QuoteSubmissionResponseStatus::Ready,
        "failed" => QuoteSubmissionResponseStatus::Failed,
        _ => QuoteSubmissionResponseStatus::Queued,
    }
}

fn detail_status(status: &str) -> Result<QuoteDetailStatus, ApiError> {
    match status {
        "queued" => Ok(QuoteDetailStatus::Queued),
        "analyzing" => Ok(QuoteDetailStatus::Analyzing),
        "completed" | "ready" => Ok(QuoteDetailStatus::Ready),
        "failed" => Ok(QuoteDetailStatus::Failed),
        _ => Err(invalid_stored_status()),
    }
}

fn quote_summary(record: &QuoteRecord) -> Result<QuoteSummary, ApiError> {
    let status = match record.status.as_str() {
        "queued" => QuoteSummaryStatus::Queued,
        "analyzing" => QuoteSummaryStatus::Analyzing,
        "completed" | "ready" => QuoteSummaryStatus::Ready,
        "failed" => QuoteSummaryStatus::Failed,
        _ => return Err(invalid_stored_status()),
    };
    Ok(QuoteSummary {
        quote_id: record.quote_id.to_string(),
        status,
        organization_name: record.request.organization_name.clone(),
        frameworks: record.request.frameworks.clone(),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        estimate: quote_estimate(record)?,
    })
}

fn event_stage(status: &str) -> Result<QuoteStatusEventStage, ApiError> {
    match status {
        "queued" => Ok(QuoteStatusEventStage::Queued),
        "loading_context" => Ok(QuoteStatusEventStage::LoadingContext),
        "analyzing" => Ok(QuoteStatusEventStage::Analyzing),
        "validating" => Ok(QuoteStatusEventStage::Validating),
        "completed" | "ready" => Ok(QuoteStatusEventStage::Ready),
        "failed" => Ok(QuoteStatusEventStage::Failed),
        _ => Err(invalid_stored_status()),
    }
}

fn quote_failure_problem(record: &QuoteRecord) -> Option<QuoteProblem> {
    (record.status == "failed").then(|| QuoteProblem {
        code: "analysis_failed".into(),
        message: "Quote analysis failed; the request may be retried.".into(),
        request_id: Uuid::new_v4().to_string(),
    })
}

fn quote_estimate(record: &QuoteRecord) -> Result<Option<QuoteEstimate>, ApiError> {
    if !matches!(record.status.as_str(), "completed" | "ready") {
        return Ok(None);
    }
    let object = record
        .analysis
        .as_ref()
        .and_then(JsonValue::as_object)
        .ok_or_else(invalid_analysis)?;
    let fee_low = required_u64(object, "estimated_total_fee_low")?;
    let fee_high = required_u64(object, "estimated_total_fee_high")?;
    let lower_bound_cents = cents(fee_low)?;
    let upper_bound_cents = cents(fee_high)?;
    let duration_weeks_low = integer(object, "estimated_total_weeks_low")?;
    let duration_weeks_high = integer(object, "estimated_total_weeks_high")?;
    if lower_bound_cents > upper_bound_cents
        || duration_weeks_low < 1
        || duration_weeks_low > duration_weeks_high
    {
        return Err(invalid_analysis());
    }

    let services = object
        .get("recommended_services")
        .and_then(JsonValue::as_array)
        .ok_or_else(invalid_analysis)?;
    let mut next_steps = services
        .iter()
        .filter_map(|service| {
            let service = service.as_object()?;
            let phase = service.get("phase")?.as_str()?.trim();
            let scope = service.get("scope")?.as_str()?.trim();
            (!phase.is_empty() && !scope.is_empty()).then(|| format!("{phase}: {scope}"))
        })
        .take(25)
        .collect::<Vec<_>>();
    if next_steps.is_empty() {
        next_steps.push("Review the preliminary estimate with Canonical.".into());
    }

    let mut gaps = strings(object.get("risks"))?;
    gaps.extend(strings(object.get("missing_information"))?);
    gaps.truncate(50);

    Ok(Some(QuoteEstimate {
        quote_id: record.quote_id.to_string(),
        status: "ready".into(),
        currency: required_string(object, "currency")?.into(),
        lower_bound_cents,
        upper_bound_cents,
        duration_weeks_low,
        duration_weeks_high,
        confidence: aggregate_confidence(services),
        summary: required_string(object, "summary")?.into(),
        assumptions: strings(object.get("assumptions"))?,
        gaps,
        next_steps,
        frameworks: record.request.frameworks.clone(),
        model: record.gemini_model.clone(),
        context_version: format!(
            "context-record:{}@{}",
            record.context_record_id, record.created_at
        ),
        created_at: record.updated_at.clone(),
    }))
}

fn cents(value: u64) -> Result<i64, ApiError> {
    value
        .checked_mul(100)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(invalid_analysis)
}

fn integer(object: &Map<String, JsonValue>, field: &str) -> Result<i64, ApiError> {
    i64::try_from(required_u64(object, field)?).map_err(|_| invalid_analysis())
}

fn required_u64(object: &Map<String, JsonValue>, field: &str) -> Result<u64, ApiError> {
    object
        .get(field)
        .and_then(JsonValue::as_u64)
        .ok_or_else(invalid_analysis)
}

fn required_string<'a>(
    object: &'a Map<String, JsonValue>,
    field: &str,
) -> Result<&'a str, ApiError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_analysis)
}

fn strings(value: Option<&JsonValue>) -> Result<Vec<String>, ApiError> {
    value
        .and_then(JsonValue::as_array)
        .ok_or_else(invalid_analysis)?
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(invalid_analysis)
        })
        .collect()
}

fn aggregate_confidence(services: &[JsonValue]) -> String {
    let confidence = services
        .iter()
        .filter_map(|service| service.get("confidence").and_then(JsonValue::as_str))
        .collect::<Vec<_>>();
    if confidence.contains(&"low") {
        "low".into()
    } else if !confidence.is_empty() && confidence.iter().all(|value| *value == "high") {
        "high".into()
    } else {
        "medium".into()
    }
}

fn invalid_stored_status() -> ApiError {
    ApiError::service_unavailable(
        "storage_unavailable",
        "quote storage contains an invalid status",
    )
}

fn invalid_analysis() -> ApiError {
    ApiError::service_unavailable(
        "storage_unavailable",
        "a ready quote contains an invalid estimate",
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_quote_request, quote_detail, quote_status_event};
    use crate::{QuoteEventRecord, QuoteRecord};
    use serde_json::{json, Value};
    use uuid::Uuid;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/quote-v1/create-request.json")).unwrap()
    }

    fn record() -> QuoteRecord {
        let request = parse_quote_request(fixture()).unwrap().wire;
        QuoteRecord {
            analysis: Some(json!({
                "summary": "Preliminary readiness estimate.",
                "currency": "USD",
                "estimated_total_fee_low": 25000,
                "estimated_total_fee_high": 45000,
                "estimated_total_weeks_low": 8,
                "estimated_total_weeks_high": 16,
                "recommended_services": [{
                    "phase": "Readiness",
                    "scope": "Confirm the evidence boundary",
                    "confidence": "medium"
                }],
                "assumptions": ["One production environment."],
                "risks": ["Risk assessment is incomplete."],
                "missing_information": []
            })),
            context_record_id: Uuid::nil(),
            error_code: None,
            gemini_model: "gemini-3.6-flash".into(),
            owner_subject: "owner-a".into(),
            persistence: "postgres".into(),
            quote_id: Uuid::parse_str("018f47c4-32a9-7c31-8f27-f3357f781001").unwrap(),
            request,
            status: "completed".into(),
            created_at: "2026-08-06T04:30:00Z".into(),
            updated_at: "2026-08-06T04:31:12Z".into(),
        }
    }

    #[test]
    fn current_request_fixture_is_accepted() {
        let accepted = parse_quote_request(fixture()).unwrap();
        assert_eq!(accepted.wire.organization_name, "Example Incorporated");
        assert_eq!(accepted.wire.context_key.as_deref(), Some("quote-analysis"));
    }

    #[test]
    fn unknown_fields_fail_closed() {
        let mut payload = fixture();
        payload["markdown_context"] = json!("untrusted");
        assert!(parse_quote_request(payload).is_err());
    }

    #[test]
    fn detail_and_event_follow_current_public_contract() {
        let record = record();
        let detail = quote_detail(&record).unwrap();
        assert_eq!(
            detail.status,
            canonical_lib::interfaces::QuoteDetailStatus::Ready
        );
        assert_eq!(detail.estimate.unwrap().lower_bound_cents, 2_500_000);

        let event = quote_status_event(
            &QuoteEventRecord {
                owner_subject: "owner-a".into(),
                quote_id: record.quote_id,
                sequence: 3,
                status: "completed".into(),
                occurred_at: "2026-08-06T04:31:12Z".into(),
            },
            &record,
        )
        .unwrap();
        assert!(event.terminal);
        assert_eq!(event.occurred_at, "2026-08-06T04:31:12Z");
    }
}
