//! Tenant-scoped GRC action queue.
//!
//! This turns the persistence model into an actionable product surface: one
//! authenticated feed for due reviews, remediation, approvals, evidence
//! requests, and failed automation. PostgreSQL RLS remains the primary tenant
//! boundary; every query also carries the exact tenant id as defense in depth.

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::get,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, Statement, TransactionTrait,
};
use serde::Serialize;
use subtle::ConstantTimeEq;
use tracing::error;
use uuid::Uuid;

use crate::{
    ApiError, AppState, INTERNAL_TOKEN_HEADER, SUBJECT_HEADER, valid_subject,
};

const TENANT_HEADER: &str = "x-canonical-tenant-id";
const GRC_READ_SCOPE: &str = "grc:read";
const MAX_WORK_ITEMS: usize = 500;

const WORK_QUEUE_SQL: &str = r#"
WITH work_items AS (
    SELECT
        'vendor_assessment'::text AS kind,
        assessment.id,
        assessment.vendor_id AS parent_id,
        assessment.state,
        left(vendor.display_name || ': ' || assessment.assessment_kind, 240) AS title,
        assessment.next_review_at AS due_at,
        CASE
            WHEN assessment.residual_risk_basis_points >= 7500 THEN 'high'
            WHEN assessment.next_review_at IS NOT NULL AND assessment.next_review_at < now() THEN 'high'
            ELSE 'medium'
        END::text AS priority,
        'review_vendor_assessment'::text AS action
    FROM vendor_assessments AS assessment
    JOIN vendors AS vendor
      ON vendor.tenant_id = assessment.tenant_id
     AND vendor.id = assessment.vendor_id
    WHERE assessment.tenant_id = $1
      AND (
        assessment.state IN ('planned', 'collecting', 'in_review')
        OR assessment.next_review_at <= now() + interval '14 days'
      )

    UNION ALL

    SELECT
        'access_review_item'::text,
        item.id,
        item.campaign_id,
        CASE
            WHEN item.decision = 'pending' THEN 'pending_decision'
            ELSE item.remediation_state
        END::text,
        left(item.principal_key || ' / ' || item.entitlement_key, 240),
        campaign.due_at,
        CASE
            WHEN campaign.due_at IS NOT NULL AND campaign.due_at < now() THEN 'high'
            WHEN item.remediation_state = 'failed' THEN 'high'
            ELSE 'medium'
        END::text,
        CASE
            WHEN item.decision = 'pending' THEN 'decide_access'
            ELSE 'remediate_access'
        END::text
    FROM access_review_items AS item
    JOIN access_review_campaigns AS campaign
      ON campaign.tenant_id = item.tenant_id
     AND campaign.id = item.campaign_id
    WHERE item.tenant_id = $1
      AND (
        item.decision = 'pending'
        OR item.remediation_state IN ('pending', 'in_progress', 'failed')
      )
      AND campaign.state NOT IN ('complete', 'cancelled')

    UNION ALL

    SELECT
        'security_questionnaire'::text,
        questionnaire.id,
        questionnaire.vendor_id,
        questionnaire.state,
        left(questionnaire.title, 240),
        questionnaire.due_at,
        CASE
            WHEN questionnaire.due_at IS NOT NULL AND questionnaire.due_at < now() THEN 'high'
            WHEN questionnaire.state = 'in_review' THEN 'medium'
            ELSE 'low'
        END::text,
        CASE
            WHEN questionnaire.state = 'in_review' THEN 'approve_questionnaire'
            ELSE 'complete_questionnaire'
        END::text
    FROM security_questionnaires AS questionnaire
    WHERE questionnaire.tenant_id = $1
      AND questionnaire.state IN ('draft', 'open', 'in_progress', 'in_review', 'received')

    UNION ALL

    SELECT
        'risk'::text,
        risk.id,
        risk.scope_id,
        risk.state,
        left(risk.title, 240),
        risk.review_due_at,
        CASE
            WHEN risk.inherent_likelihood * risk.inherent_impact >= 16 THEN 'high'
            WHEN risk.review_due_at IS NOT NULL AND risk.review_due_at < now() THEN 'high'
            WHEN risk.inherent_likelihood * risk.inherent_impact >= 9 THEN 'medium'
            ELSE 'low'
        END::text,
        CASE
            WHEN NOT EXISTS (
                SELECT 1
                FROM risk_treatments AS treatment
                WHERE treatment.tenant_id = risk.tenant_id
                  AND treatment.risk_id = risk.id
                  AND treatment.state IN ('planned', 'in_progress', 'implemented')
            ) THEN 'choose_risk_treatment'
            ELSE 'review_risk'
        END::text
    FROM risks AS risk
    WHERE risk.tenant_id = $1
      AND risk.state IN ('open', 'monitoring')
      AND (
        risk.review_due_at <= now() + interval '14 days'
        OR NOT EXISTS (
            SELECT 1
            FROM risk_treatments AS treatment
            WHERE treatment.tenant_id = risk.tenant_id
              AND treatment.risk_id = risk.id
              AND treatment.state IN ('planned', 'in_progress', 'implemented')
        )
      )

    UNION ALL

    SELECT
        'trust_center_access'::text,
        request.id,
        request.trust_center_id,
        CASE
            WHEN request.nda_state = 'pending' THEN 'nda_pending'
            ELSE request.state
        END::text,
        left(
            request.requester_name ||
            CASE
                WHEN request.requester_company IS NULL THEN ''
                ELSE ' — ' || request.requester_company
            END,
            240
        ),
        request.expires_at,
        CASE
            WHEN request.requested_at < now() - interval '2 days' THEN 'high'
            ELSE 'medium'
        END::text,
        CASE
            WHEN request.nda_state = 'pending' THEN 'complete_nda'
            ELSE 'decide_trust_access'
        END::text
    FROM trust_center_access_requests AS request
    WHERE request.tenant_id = $1
      AND (
        request.state = 'pending'
        OR request.nda_state = 'pending'
      )

    UNION ALL

    SELECT
        'audit_evidence_request'::text,
        request.id,
        request.engagement_id,
        request.state,
        left(request.title, 240),
        request.due_at,
        CASE
            WHEN request.due_at IS NOT NULL AND request.due_at < now() THEN 'high'
            WHEN request.state = 'rejected' THEN 'high'
            ELSE 'medium'
        END::text,
        CASE
            WHEN request.state = 'rejected' THEN 'resubmit_evidence'
            ELSE 'submit_evidence'
        END::text
    FROM audit_evidence_requests AS request
    WHERE request.tenant_id = $1
      AND request.state IN ('open', 'in_progress', 'rejected')

    UNION ALL

    SELECT
        'policy_review'::text,
        policy.id,
        NULL::uuid,
        policy.state,
        left(policy.title, 240),
        policy.next_review_at,
        CASE
            WHEN policy.next_review_at IS NOT NULL AND policy.next_review_at < now() THEN 'high'
            ELSE 'medium'
        END::text,
        CASE
            WHEN policy.state = 'draft' THEN 'publish_policy'
            ELSE 'review_policy'
        END::text
    FROM policies AS policy
    WHERE policy.tenant_id = $1
      AND (
        policy.state = 'draft'
        OR (
            policy.state = 'active'
            AND policy.next_review_at <= now() + interval '14 days'
        )
      )

    UNION ALL

    SELECT
        'training_assignment'::text,
        assignment.id,
        assignment.course_id,
        assignment.state,
        left(course.title, 240),
        assignment.due_at,
        CASE
            WHEN assignment.state = 'overdue'
              OR (assignment.due_at IS NOT NULL AND assignment.due_at < now())
                THEN 'high'
            ELSE 'low'
        END::text,
        'complete_training'::text
    FROM training_assignments AS assignment
    JOIN training_courses AS course
      ON course.tenant_id = assignment.tenant_id
     AND course.id = assignment.course_id
    WHERE assignment.tenant_id = $1
      AND assignment.state IN ('assigned', 'in_progress', 'overdue')
      AND (
        assignment.state = 'overdue'
        OR assignment.due_at <= now() + interval '7 days'
      )

    UNION ALL

    SELECT
        'automation_run'::text,
        run.id,
        run.job_id,
        run.state,
        left(job.name, 240),
        NULL::timestamptz,
        CASE
            WHEN run.state = 'failed' THEN 'high'
            ELSE 'medium'
        END::text,
        'inspect_automation_run'::text
    FROM automation_runs AS run
    JOIN automation_jobs AS job
      ON job.tenant_id = run.tenant_id
     AND job.id = run.job_id
    WHERE run.tenant_id = $1
      AND run.state IN ('failed', 'partial')
      AND run.started_at >= now() - interval '30 days'

    UNION ALL

    SELECT
        'ai_system_review'::text,
        system.id,
        system.asset_id,
        system.deployment_state,
        left(system.name, 240),
        system.next_review_at,
        CASE
            WHEN system.risk_tier = 'prohibited' THEN 'high'
            WHEN system.risk_tier = 'high' THEN 'high'
            ELSE 'medium'
        END::text,
        'review_ai_system'::text
    FROM ai_systems AS system
    WHERE system.tenant_id = $1
      AND system.deployment_state IN ('discovered', 'proposed', 'approved', 'active')
      AND (
        system.risk_tier IN ('high', 'prohibited')
        OR system.next_review_at <= now() + interval '30 days'
      )
)
SELECT
    kind,
    id,
    parent_id,
    state,
    title,
    CASE
        WHEN due_at IS NULL THEN NULL
        ELSE to_char(
            due_at AT TIME ZONE 'UTC',
            'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'
        )
    END AS due_at_text,
    priority,
    action,
    count(*) OVER ()::bigint AS total_count
FROM work_items
ORDER BY
    CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END,
    due_at NULLS LAST,
    kind,
    id
LIMIT 500
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
struct TenantIdentity {
    subject: String,
    tenant_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkQueueResponse {
    schema_version: &'static str,
    tenant_id: Uuid,
    total_matching: u64,
    returned: usize,
    truncated: bool,
    items: Vec<WorkItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItem {
    kind: String,
    id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    state: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_at: Option<String>,
    priority: String,
    action: String,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/grc/work-items", get(list_work_items))
        .route("/api/v1/grc/work-items", get(list_work_items))
}

async fn list_work_items(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WorkQueueResponse>, ApiError> {
    let identity = authenticate_tenant(&headers, &state, &[GRC_READ_SCOPE]).await?;
    let database = state.database.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(
            "grc_storage_unavailable",
            "GRC persistence is not configured",
        )
    })?;
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(ApiError::service_unavailable(
            "grc_storage_unavailable",
            "GRC persistence requires PostgreSQL",
        ));
    }

    let transaction = database.begin().await.map_err(database_error)?;
    set_tenant(&transaction, identity.tenant_id).await?;
    let rows = transaction
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            WORK_QUEUE_SQL,
            [identity.tenant_id.into()],
        ))
        .await
        .map_err(database_error)?;

    let mut total_matching = 0_u64;
    let mut items = Vec::with_capacity(rows.len().min(MAX_WORK_ITEMS));
    for row in rows {
        let total = row
            .try_get::<i64>("", "total_count")
            .map_err(database_error)?;
        total_matching = u64::try_from(total).map_err(|_| {
            ApiError::service_unavailable(
                "grc_storage_unavailable",
                "GRC work queue returned an invalid count",
            )
        })?;
        items.push(WorkItem {
            kind: row.try_get("", "kind").map_err(database_error)?,
            id: row.try_get("", "id").map_err(database_error)?,
            parent_id: row.try_get("", "parent_id").map_err(database_error)?,
            state: row.try_get("", "state").map_err(database_error)?,
            title: row.try_get("", "title").map_err(database_error)?,
            due_at: row.try_get("", "due_at_text").map_err(database_error)?,
            priority: row.try_get("", "priority").map_err(database_error)?,
            action: row.try_get("", "action").map_err(database_error)?,
        });
    }
    transaction.commit().await.map_err(database_error)?;

    let returned = items.len();
    Ok(Json(WorkQueueResponse {
        schema_version: "canonical.grc.work-queue.v1",
        tenant_id: identity.tenant_id,
        total_matching,
        returned,
        truncated: total_matching > returned as u64,
        items,
    }))
}

async fn set_tenant(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
) -> Result<(), ApiError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.tenant_id', $1, true)",
            [tenant_id.to_string().into()],
        ))
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn authenticate_tenant(
    headers: &HeaderMap,
    state: &AppState,
    required_scopes: &[&str],
) -> Result<TenantIdentity, ApiError> {
    let supplied_token = headers
        .get(INTERNAL_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let supplied_subject = headers.get(SUBJECT_HEADER);
    let supplied_tenant = headers.get(TENANT_HEADER);

    if supplied_token.is_some() || supplied_subject.is_some() || supplied_tenant.is_some() {
        let token_is_valid = supplied_token
            .map(|token| bool::from(token.as_bytes().ct_eq(state.internal_auth_token.as_bytes())))
            .unwrap_or(false);
        if !token_is_valid {
            return Err(unauthorized());
        }
        let subject = supplied_subject
            .and_then(|value| value.to_str().ok())
            .and_then(valid_subject)
            .ok_or_else(unauthorized)?;
        let tenant_id = supplied_tenant
            .and_then(|value| value.to_str().ok())
            .and_then(parse_tenant_id)
            .ok_or_else(unauthorized)?;
        return Ok(TenantIdentity {
            subject: subject.to_owned(),
            tenant_id,
        });
    }

    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 16 * 1024)
        .ok_or_else(unauthorized)?;
    let authority = state.shared_auth.as_ref().ok_or_else(unauthorized)?;
    let introspection = authority
        .client
        .introspect_with_requirements(bearer, &authority.audience, required_scopes)
        .await
        .map_err(|_| unauthorized())?;
    if !introspection.active {
        return Err(unauthorized());
    }
    let subject = introspection
        .sub
        .as_deref()
        .and_then(valid_subject)
        .ok_or_else(unauthorized)?;
    let tenant_id = introspection
        .rest
        .get("tenant_id")
        .or_else(|| introspection.rest.get("tenantId"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_tenant_id)
        .ok_or_else(unauthorized)?;

    Ok(TenantIdentity {
        subject: subject.to_owned(),
        tenant_id,
    })
}

fn parse_tenant_id(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value != value.to_ascii_lowercase() {
        return None;
    }
    Uuid::parse_str(value).ok()
}

fn unauthorized() -> ApiError {
    ApiError::unauthorized(
        "unauthorized",
        "tenant-scoped GRC authentication failed",
    )
}

fn database_error(error: sea_orm::DbErr) -> ApiError {
    error!(error_code = "grc_database_operation_failed", %error);
    ApiError::service_unavailable(
        "grc_storage_unavailable",
        "GRC persistence is temporarily unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_ids_are_canonical_uuid_values() {
        assert!(parse_tenant_id("6ba7b810-9dad-41d1-80b4-00c04fd430c8").is_some());
        assert!(parse_tenant_id("6BA7B810-9DAD-41D1-80B4-00C04FD430C8").is_none());
        assert!(parse_tenant_id("../../tenant").is_none());
    }

    #[test]
    fn work_queue_covers_the_major_grc_action_surfaces() {
        for required in [
            "vendor_assessments",
            "access_review_items",
            "security_questionnaires",
            "risks",
            "trust_center_access_requests",
            "audit_evidence_requests",
            "policies",
            "training_assignments",
            "automation_runs",
            "ai_systems",
        ] {
            assert!(WORK_QUEUE_SQL.contains(required), "{required}");
        }
        assert!(WORK_QUEUE_SQL.contains("tenant_id = $1"));
        assert!(WORK_QUEUE_SQL.contains("LIMIT 500"));
        assert!(!WORK_QUEUE_SQL.contains("SELECT *"));
    }
}
