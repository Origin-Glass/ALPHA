use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum GovernanceError {
    Auth(AuthError),
    InvalidInput(&'static str),
    Forbidden,
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}

impl IntoResponse for GovernanceError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "governance_forbidden",
                "해당 검토 권한이 없습니다",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "governance_not_found",
                "검토 대상을 찾을 수 없습니다",
            ),
            Self::Conflict(message) => (StatusCode::CONFLICT, "governance_gate_rejected", message),
            Self::Database(error) => {
                tracing::error!(%error, "콘텐츠 거버넌스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
        };
        (
            status,
            Json(serde_json::json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

impl From<AuthError> for GovernanceError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for GovernanceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub async fn has_capability(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    capability: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles ur JOIN role_capabilities rc ON rc.role = ur.role WHERE ur.user_id = $1 AND rc.capability = $2)",
    )
    .bind(user_id)
    .bind(capability)
    .fetch_one(pool)
    .await
}

async fn require_capability(
    state: &AppState,
    user_id: Uuid,
    capability: &str,
) -> Result<(), GovernanceError> {
    if !has_capability(state.pool(), user_id, capability).await? {
        return Err(GovernanceError::Forbidden);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RightsRequest {
    basis: String,
    evidence: String,
    commercial_use_allowed: bool,
    redistribution_allowed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    note: String,
}

pub async fn record_rights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<RightsRequest>,
) -> Result<Json<Value>, GovernanceError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    require_capability(&state, user_id, "content.create").await?;
    if !matches!(
        request.basis.as_str(),
        "original" | "licensed" | "public_domain"
    ) || !(3..=2000).contains(&request.evidence.trim().chars().count())
    {
        return Err(GovernanceError::InvalidInput("권리 근거를 확인해 주세요"));
    }
    let mut transaction = state.pool().begin().await?;
    let (problem_id, revision_id, author_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT id, current_revision_id, created_by FROM problems WHERE slug = $1 AND status = 'draft' FOR UPDATE",
    )
    .bind(&slug)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(GovernanceError::NotFound)?;
    if author_id != user_id {
        return Err(GovernanceError::Forbidden);
    }
    sqlx::query(
        r#"INSERT INTO content_governance (
               problem_revision_id, problem_id, author_user_id, rights_basis, rights_evidence,
               commercial_use_allowed, redistribution_allowed
           ) VALUES ($1,$2,$3,$4,$5,$6,$7)
           ON CONFLICT (problem_revision_id) DO UPDATE SET
               rights_basis = EXCLUDED.rights_basis, rights_evidence = EXCLUDED.rights_evidence,
               commercial_use_allowed = EXCLUDED.commercial_use_allowed,
               redistribution_allowed = EXCLUDED.redistribution_allowed,
               content_reviewed_by = NULL, content_review_note = NULL, content_reviewed_at = NULL,
               rights_reviewed_by = NULL, rights_review_note = NULL, rights_reviewed_at = NULL,
               updated_at = now()"#,
    )
    .bind(revision_id)
    .bind(problem_id)
    .bind(author_id)
    .bind(&request.basis)
    .bind(request.evidence.trim())
    .bind(request.commercial_use_allowed)
    .bind(request.redistribution_allowed)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content.rights.recorded', 'problem_revision', $2, jsonb_build_object('basis', $3::text))")
        .bind(user_id).bind(revision_id.to_string()).bind(&request.basis).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"status": "rights_recorded"})))
}

async fn review(
    state: AppState,
    headers: HeaderMap,
    slug: String,
    request: ReviewRequest,
    rights: bool,
) -> Result<Json<Value>, GovernanceError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    require_capability(
        &state,
        user_id,
        if rights {
            "rights.review"
        } else {
            "content.review"
        },
    )
    .await?;
    if !(3..=1000).contains(&request.note.trim().chars().count()) {
        return Err(GovernanceError::InvalidInput(
            "검토 근거를 3자 이상 입력해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let (revision_id, author_id, content_reviewer, usable_rights): (
        Uuid,
        Uuid,
        Option<Uuid>,
        bool,
    ) = sqlx::query_as(
        r#"SELECT governance.problem_revision_id, governance.author_user_id,
                  governance.content_reviewed_by,
                  governance.commercial_use_allowed AND governance.redistribution_allowed
           FROM content_governance governance
           JOIN problems problem ON problem.id = governance.problem_id
           WHERE problem.slug = $1 AND problem.status = 'draft'
             AND problem.current_revision_id = governance.problem_revision_id
           FOR UPDATE OF governance, problem"#,
    )
    .bind(&slug)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(GovernanceError::Conflict("승인된 권리 기록이 필요합니다"))?;
    if author_id == user_id || (rights && content_reviewer == Some(user_id)) {
        return Err(GovernanceError::Forbidden);
    }
    if rights && !usable_rights {
        return Err(GovernanceError::Conflict(
            "상업 이용과 재배포가 모두 허용되어야 합니다",
        ));
    }
    let (action, status) = if rights {
        sqlx::query("UPDATE content_governance SET rights_reviewed_by = $2, rights_review_note = $3, rights_reviewed_at = now(), updated_at = now() WHERE problem_revision_id = $1")
            .bind(revision_id).bind(user_id).bind(request.note.trim()).execute(&mut *transaction).await?;
        ("content.rights.approved", "rights_approved")
    } else {
        sqlx::query("UPDATE content_governance SET content_reviewed_by = $2, content_review_note = $3, content_reviewed_at = now(), updated_at = now() WHERE problem_revision_id = $1")
            .bind(revision_id).bind(user_id).bind(request.note.trim()).execute(&mut *transaction).await?;
        ("content.review.approved", "content_approved")
    };
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, $2, 'problem_revision', $3)")
        .bind(user_id).bind(action).bind(revision_id.to_string()).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"status": status})))
}

pub async fn approve_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<ReviewRequest>,
) -> Result<Json<Value>, GovernanceError> {
    review(state, headers, slug, request, false).await
}

pub async fn approve_rights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<ReviewRequest>,
) -> Result<Json<Value>, GovernanceError> {
    review(state, headers, slug, request, true).await
}

pub async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<Value>, GovernanceError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    require_capability(&state, user_id, "content.publish").await?;
    let mut transaction = state.pool().begin().await?;
    let row: Option<(
        Uuid,
        Uuid,
        Option<bool>,
        Option<bool>,
        Option<Uuid>,
        Option<Uuid>,
    )> = sqlx::query_as(
        r#"SELECT problem.id, problem.current_revision_id,
                  governance.commercial_use_allowed, governance.redistribution_allowed,
                  governance.content_reviewed_by, governance.rights_reviewed_by
           FROM problems problem
           LEFT JOIN content_governance governance
             ON governance.problem_revision_id = problem.current_revision_id
           WHERE problem.slug = $1 AND problem.status = 'draft'
           FOR UPDATE OF problem"#,
    )
    .bind(&slug)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((
        problem_id,
        revision_id,
        commercial,
        redistribution,
        content_reviewer,
        rights_reviewer,
    )) = row
    else {
        return Err(GovernanceError::Conflict("게시 요건을 충족하지 못했습니다"));
    };
    if commercial != Some(true)
        || redistribution != Some(true)
        || content_reviewer.is_none()
        || rights_reviewer.is_none()
        || content_reviewer == rights_reviewer
    {
        return Err(GovernanceError::Conflict(
            "권리 및 분리 검토 승인이 모두 필요합니다",
        ));
    }
    sqlx::query("UPDATE problems SET status = 'published', published_at = now(), updated_at = now() WHERE id = $1")
        .bind(problem_id).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content.published', 'problem', $2, jsonb_build_object('revision_id', $3::text))")
        .bind(user_id).bind(problem_id.to_string()).bind(revision_id.to_string()).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"status": "published"})))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequestInput {
    kind: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DataRequestView {
    id: Uuid,
    kind: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
}

pub async fn create_data_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DataRequestInput>,
) -> Result<(StatusCode, Json<Value>), GovernanceError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    if !matches!(request.kind.as_str(), "export" | "delete") {
        return Err(GovernanceError::InvalidInput("요청 종류를 확인해 주세요"));
    }
    let mut transaction = state.pool().begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO data_requests (user_id, kind) VALUES ($1, $2) RETURNING id",
    )
    .bind(user_id)
    .bind(&request.kind)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'data_request.created', 'data_request', $2, jsonb_build_object('kind', $3::text))")
        .bind(user_id).bind(id.to_string()).bind(&request.kind).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "status": "pending"})),
    ))
}

pub async fn data_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<DataRequestView>, GovernanceError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    let item = sqlx::query_as(
        "SELECT id, kind, status, created_at FROM data_requests WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(GovernanceError::NotFound)?;
    Ok(Json(item))
}

pub async fn cancel_data_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, GovernanceError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    crate::auth::require_current_policy(&state, user_id).await?;
    let mut transaction = state.pool().begin().await?;
    let updated = sqlx::query(
        "UPDATE data_requests SET status = 'cancelled', updated_at = now() WHERE id = $1 AND user_id = $2 AND status = 'pending'",
    ).bind(id).bind(user_id).execute(&mut *transaction).await?;
    if updated.rows_affected() != 1 {
        return Err(GovernanceError::NotFound);
    }
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'data_request.cancelled', 'data_request', $2)")
        .bind(user_id).bind(id.to_string()).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"status": "cancelled"})))
}
