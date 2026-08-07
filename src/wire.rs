use canonical_interfaces::{
    QuoteDetail, QuoteDetailStatus, QuoteEstimate, QuoteListResponse, QuoteProblem,
    QuoteRequest as WireQuoteRequest, QuoteRetryResponse, QuoteRetryResponseStatus,
    QuoteStatusEvent as WireQuoteStatusEvent, QuoteStatusEventStage, QuoteSubmissionResponse,
    QuoteSubmissionResponseStatus, QuoteSummary, QuoteSummaryStatus,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value as JsonValue;

use crate::{ApiError, CreateQuoteRequest, OrganizationInput, QuoteEvent, QuoteRecord};

pub(crate) fn normalize_request(wire: WireQuoteRequest) -> Result<CreateQuoteRequest, ApiError> {
    let employee_count = u32::try_from(wire.employee_count).map_err(|_| {
        ApiError::bad_request(
            "invalid_employee_count",
            "employeeCount must be between 1 and 1000000",
        )
    })?;
    let organization = OrganizationInput {
        employee_count,
        industry: "not_specified".into(),
        legal_name: wire.organization_name.clone(),
    };
    let request = CreateQuoteRequest {
        legacy_context_record_id: None,
        frameworks: wire.frameworks.clone(),
        markdown_context: String::new(),
        notes: wire.notes.clone(),
        organization,
        target_date: wire.target_date.clone(),
        wire,
    };
    request.validate_and_normalize()
}

pub(crate) fn now() -> DateTime<Utc> {
    Utc::now()
}

pub(crate) fn submission(record: &QuoteRecord) -> QuoteSubmissionResponse {
    QuoteSubmissionResponse {
        quote_id: record.quote_id.to_string(),
        status: submission_status(&record.status),
        stream_url: events_url(record),
        created_at: timestamp(record.created_at),
    }
}

pub(crate) fn detail(record: &QuoteRecord) -> QuoteDetail {
    let estimate = estimate(record);
    QuoteDetail {
        quote_id: record.quote_id.to_string(),
        status: detail_status(&record.status),
        request: record.request.clone(),
        events_url: events_url(record),
        created_at: timestamp(record.created_at),
        updated_at: timestamp(record.updated_at),
        estimate,
        problem: problem(record),
    }
}

pub(crate) fn list(records: Vec<QuoteRecord>) -> QuoteListResponse {
    QuoteListResponse {
        quotes: records.iter().map(summary).collect(),
        next_cursor: None,
    }
}

pub(crate) fn retry(record: &QuoteRecord) -> QuoteRetryResponse {
    QuoteRetryResponse {
        quote_id: record.quote_id.to_string(),
        status: QuoteRetryResponseStatus::Queued,
        stream_url: events_url(record),
        updated_at: timestamp(record.updated_at),
    }
}

pub(crate) fn event(event: &QuoteEvent) -> WireQuoteStatusEvent {
    WireQuoteStatusEvent {
        quote_id: event.quote_id.to_string(),
        sequence: event.sequence.to_string(),
        stage: event_stage(&event.status),
        message: event_message(&event.status).into(),
        terminal: matches!(event.status.as_str(), "ready" | "failed"),
        estimate: event.estimate.clone(),
        occurred_at: timestamp(event.occurred_at),
        problem: event.problem.clone(),
    }
}

fn summary(record: &QuoteRecord) -> QuoteSummary {
    QuoteSummary {
        quote_id: record.quote_id.to_string(),
        status: summary_status(&record.status),
        organization_name: record.organization_name.clone(),
        frameworks: record.frameworks.clone(),
        created_at: timestamp(record.created_at),
        updated_at: timestamp(record.updated_at),
        estimate: estimate(record),
    }
}

fn estimate(record: &QuoteRecord) -> Option<QuoteEstimate> {
    if record.status != "ready" {
        return None;
    }
    let analysis = record.analysis.as_ref()?.as_object()?;
    let lower_dollars = u64_field(analysis.get("estimated_total_fee_low")?)?;
    let upper_dollars = u64_field(analysis.get("estimated_total_fee_high")?)?;
    let weeks_low = u64_field(analysis.get("estimated_total_weeks_low")?)?;
    let weeks_high = u64_field(analysis.get("estimated_total_weeks_high")?)?;
    let lower_bound_cents = i64::try_from(lower_dollars.checked_mul(100)?).ok()?;
    let upper_bound_cents = i64::try_from(upper_dollars.checked_mul(100)?).ok()?;
    let duration_weeks_low = i64::try_from(weeks_low).ok()?;
    let duration_weeks_high = i64::try_from(weeks_high).ok()?;
    let summary = analysis.get("summary")?.as_str()?.to_owned();
    let assumptions = string_array(analysis.get("assumptions"));
    let mut gaps = string_array(analysis.get("risks"));
    gaps.extend(string_array(analysis.get("missing_information")));
    let next_steps = analysis
        .get("recommended_services")
        .and_then(JsonValue::as_array)
        .map(|services| {
            services
                .iter()
                .filter_map(|service| service.get("scope").and_then(JsonValue::as_str))
                .take(25)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let confidence = analysis
        .get("recommended_services")
        .and_then(JsonValue::as_array)
        .map(|services| {
            let values: Vec<_> = services
                .iter()
                .filter_map(|service| service.get("confidence").and_then(JsonValue::as_str))
                .collect();
            if values.contains(&"low") {
                "low"
            } else if !values.is_empty() && values.iter().all(|value| *value == "high") {
                "high"
            } else {
                "medium"
            }
        })
        .unwrap_or("medium")
        .to_owned();

    Some(QuoteEstimate {
        quote_id: record.quote_id.to_string(),
        status: "ready".into(),
        currency: analysis
            .get("currency")
            .and_then(JsonValue::as_str)
            .unwrap_or("USD")
            .to_owned(),
        lower_bound_cents,
        upper_bound_cents,
        duration_weeks_low,
        duration_weeks_high,
        confidence,
        summary,
        assumptions,
        gaps,
        next_steps,
        frameworks: record.frameworks.clone(),
        model: record.gemini_model.clone(),
        context_version: record.context_record_id.to_string(),
        created_at: timestamp(record.updated_at),
    })
}

fn problem(record: &QuoteRecord) -> Option<QuoteProblem> {
    (record.status == "failed").then(|| QuoteProblem {
        code: record
            .error_code
            .clone()
            .unwrap_or_else(|| "analysis_failed".into()),
        message: "Quote analysis could not be completed; retry is available.".into(),
        request_id: record.quote_id.to_string(),
    })
}

fn events_url(record: &QuoteRecord) -> String {
    format!("/api/v1/quotes/{}/events", record.quote_id)
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn u64_field(value: &JsonValue) -> Option<u64> {
    value.as_u64()
}

fn string_array(value: Option<&JsonValue>) -> Vec<String> {
    value
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(JsonValue::as_str)
                .take(50)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn submission_status(status: &str) -> QuoteSubmissionResponseStatus {
    match status {
        "analyzing" => QuoteSubmissionResponseStatus::Analyzing,
        "ready" => QuoteSubmissionResponseStatus::Ready,
        "failed" => QuoteSubmissionResponseStatus::Failed,
        _ => QuoteSubmissionResponseStatus::Queued,
    }
}

fn summary_status(status: &str) -> QuoteSummaryStatus {
    match status {
        "analyzing" => QuoteSummaryStatus::Analyzing,
        "ready" => QuoteSummaryStatus::Ready,
        "failed" => QuoteSummaryStatus::Failed,
        _ => QuoteSummaryStatus::Queued,
    }
}

fn detail_status(status: &str) -> QuoteDetailStatus {
    match status {
        "analyzing" => QuoteDetailStatus::Analyzing,
        "ready" => QuoteDetailStatus::Ready,
        "failed" => QuoteDetailStatus::Failed,
        _ => QuoteDetailStatus::Queued,
    }
}

fn event_stage(status: &str) -> QuoteStatusEventStage {
    match status {
        "analyzing" => QuoteStatusEventStage::Analyzing,
        "ready" => QuoteStatusEventStage::Ready,
        "failed" => QuoteStatusEventStage::Failed,
        _ => QuoteStatusEventStage::Queued,
    }
}

fn event_message(status: &str) -> &'static str {
    match status {
        "analyzing" => "Analyzing the bounded questionnaire and Canonical context.",
        "ready" => "The preliminary estimate is ready for human review.",
        "failed" => "Quote analysis failed; retry may be available.",
        _ => "Quote analysis is queued.",
    }
}
