use std::collections::HashSet;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum ClassError {
    Auth(AuthError),
    InvalidInput(&'static str),
    InviteInvalid,
    RoleRequired,
    TermsRequired,
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for ClassError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::InviteInvalid => (
                StatusCode::BAD_REQUEST,
                "class_invite_invalid",
                "초대 코드가 유효하지 않거나 만료되었습니다",
            ),
            Self::RoleRequired => (
                StatusCode::FORBIDDEN,
                "instructor_required",
                "강사 또는 관리자 권한이 필요합니다",
            ),
            Self::TermsRequired => (
                StatusCode::FORBIDDEN,
                "terms_required",
                "조직 또는 학급에 참여하기 전에 이용약관에 동의해 주세요",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "class_not_found",
                "학급을 찾을 수 없습니다",
            ),
            Self::Database(error) => {
                tracing::error!(%error, "학급 데이터베이스 처리 실패");
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

impl From<AuthError> for ClassError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for ClassError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn require_terms(state: &AppState, user_id: Uuid) -> Result<(), ClassError> {
    let accepted: bool = sqlx::query_scalar(
        "SELECT terms_accepted_at IS NOT NULL FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    if !accepted {
        return Err(ClassError::TermsRequired);
    }
    Ok(())
}

async fn global_instructor(state: &AppState, user_id: Uuid) -> Result<bool, ClassError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role IN ('INSTRUCTOR', 'ADMIN'))",
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?)
}

#[derive(Debug, FromRow)]
struct InstructorAccess {
    can_invite_instructor: bool,
}

async fn require_instructor(
    pool: &PgPool,
    class_id: Uuid,
    user_id: Uuid,
) -> Result<InstructorAccess, ClassError> {
    sqlx::query_as::<_, InstructorAccess>(
        r#"
        SELECT COALESCE(organization_membership.role IN ('OWNER', 'ADMIN'), false)
                   AS can_invite_instructor
        FROM classes class
        LEFT JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = class.organization_id
         AND organization_membership.user_id = $2
        LEFT JOIN class_memberships class_membership
          ON class_membership.class_id = class.id AND class_membership.user_id = $2
        WHERE class.id = $1 AND class.status = 'active'
          AND (organization_membership.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR')
               OR class_membership.role = 'INSTRUCTOR')
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ClassError::NotFound)
}

#[derive(Debug, Serialize, FromRow)]
pub struct ClassListItem {
    id: Uuid,
    organization_name: String,
    name: String,
    viewer_role: String,
    learner_count: i64,
    active_assignment_count: i64,
}

#[derive(Debug, Serialize)]
pub struct ClassListResponse {
    items: Vec<ClassListItem>,
    can_create: bool,
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ClassListResponse>, ClassError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let items = sqlx::query_as::<_, ClassListItem>(
        r#"
        SELECT DISTINCT class.id, organization.name AS organization_name, class.name,
               CASE WHEN organization_membership.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR')
                          OR class_membership.role = 'INSTRUCTOR'
                    THEN 'INSTRUCTOR' ELSE 'LEARNER' END AS viewer_role,
               (SELECT COUNT(*) FROM class_memberships member WHERE member.class_id = class.id AND member.role = 'LEARNER') AS learner_count,
               (SELECT COUNT(*) FROM class_assignments assignment WHERE assignment.class_id = class.id AND assignment.status = 'active') AS active_assignment_count
        FROM classes class
        JOIN organizations organization ON organization.id = class.organization_id AND organization.status = 'active'
        LEFT JOIN class_memberships class_membership
          ON class_membership.class_id = class.id AND class_membership.user_id = $1
        LEFT JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = class.organization_id
         AND organization_membership.user_id = $1
        WHERE class.status = 'active'
          AND (class_membership.user_id IS NOT NULL
               OR organization_membership.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR'))
        ORDER BY organization.name, class.name
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(ClassListResponse {
        items,
        can_create: global_instructor(&state, user_id).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationRequest {
    slug: String,
    name: String,
}

pub async fn create_organization(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ClassError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    if !global_instructor(&state, user_id).await? {
        return Err(ClassError::RoleRequired);
    }
    if request.name.trim().is_empty()
        || request.name.chars().count() > 120
        || !(3..=48).contains(&request.slug.len())
        || !request
            .slug
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !request.slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(ClassError::InvalidInput(
            "조직 이름과 식별자를 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let organization_id: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (slug, name, created_by) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&request.slug)
    .bind(request.name.trim())
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, user_id, role) VALUES ($1, $2, 'OWNER')",
    )
    .bind(organization_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'organization.created', 'organization', $2)",
    )
    .bind(user_id)
    .bind(organization_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": organization_id})),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateClassRequest {
    name: String,
}

pub async fn create_class(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateClassRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ClassError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if request.name.trim().is_empty() || request.name.chars().count() > 120 {
        return Err(ClassError::InvalidInput("학급 이름을 확인해 주세요"));
    }
    let manager: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM organization_memberships WHERE organization_id = $1 AND user_id = $2 AND role IN ('OWNER', 'ADMIN', 'INSTRUCTOR'))",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !manager {
        return Err(ClassError::NotFound);
    }
    let mut transaction = state.pool().begin().await?;
    let class_id: Uuid = sqlx::query_scalar(
        "INSERT INTO classes (organization_id, name, created_by) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(organization_id)
    .bind(request.name.trim())
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO class_memberships (class_id, user_id, role) VALUES ($1, $2, 'INSTRUCTOR')",
    )
    .bind(class_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'class.created', 'class', $2)",
    )
    .bind(user_id)
    .bind(class_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": class_id})),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInvitationRequest {
    role: String,
    expires_in_days: i64,
}

#[derive(Debug, Serialize)]
pub struct InvitationResponse {
    code: String,
    join_path: String,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

fn invitation_token() -> Result<String, ClassError> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|_| ClassError::InvalidInput("초대 코드를 만들지 못했습니다"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
    Json(request): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<InvitationResponse>), ClassError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let access = require_instructor(state.pool(), class_id, user_id).await?;
    let role = match request.role.as_str() {
        "learner" => "LEARNER",
        "instructor" if access.can_invite_instructor => "INSTRUCTOR",
        "instructor" => return Err(ClassError::RoleRequired),
        _ => return Err(ClassError::InvalidInput("초대 역할을 확인해 주세요")),
    };
    if !(1..=30).contains(&request.expires_in_days) {
        return Err(ClassError::InvalidInput("초대 유효 기간은 1~30일입니다"));
    }
    let token = invitation_token()?;
    let expires_at = OffsetDateTime::now_utc() + Duration::days(request.expires_in_days);
    let mut transaction = state.pool().begin().await?;
    let invitation_id: Uuid = sqlx::query_scalar(
        "INSERT INTO class_invitations (class_id, token_hash, class_role, expires_at, created_by) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(class_id)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(role)
    .bind(expires_at)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'class.invitation.created', 'class_invitation', $2, jsonb_build_object('class_id', $3::text, 'role', $4::text))",
    )
    .bind(user_id)
    .bind(invitation_id.to_string())
    .bind(class_id.to_string())
    .bind(role)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(InvitationResponse {
            join_path: format!("/classes/join?code={token}"),
            code: token,
            expires_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptInvitationRequest {
    code: String,
}

pub async fn accept_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AcceptInvitationRequest>,
) -> Result<Json<serde_json::Value>, ClassError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    if !(20..=100).contains(&request.code.len()) {
        return Err(ClassError::InviteInvalid);
    }
    let mut transaction = state.pool().begin().await?;
    let invitation: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        r#"
        SELECT invitation.id, invitation.class_id, invitation.class_role
        FROM class_invitations invitation
        WHERE invitation.token_hash = $1 AND invitation.accepted_at IS NULL
          AND invitation.expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(Sha256::digest(request.code.as_bytes()).to_vec())
    .fetch_optional(&mut *transaction)
    .await?;
    let (invitation_id, class_id, class_role) = invitation.ok_or(ClassError::InviteInvalid)?;
    let organization_id: Uuid = sqlx::query_scalar(
        "SELECT organization_id FROM classes WHERE id = $1 AND status = 'active'",
    )
    .bind(class_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ClassError::InviteInvalid)?;
    let organization_role = if class_role == "INSTRUCTOR" {
        "INSTRUCTOR"
    } else {
        "MEMBER"
    };
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, user_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (organization_id, user_id) DO UPDATE
        SET role = CASE WHEN organization_memberships.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR')
                        THEN organization_memberships.role ELSE EXCLUDED.role END
        "#,
    )
    .bind(organization_id)
    .bind(user_id)
    .bind(organization_role)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO class_memberships (class_id, user_id, role) VALUES ($1, $2, $3)
        ON CONFLICT (class_id, user_id) DO UPDATE
        SET role = CASE WHEN class_memberships.role = 'INSTRUCTOR' THEN 'INSTRUCTOR' ELSE EXCLUDED.role END
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .bind(&class_role)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE class_invitations SET accepted_by = $2, accepted_at = now() WHERE id = $1")
        .bind(invitation_id)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'class.invitation.accepted', 'class', $2)",
    )
    .bind(user_id)
    .bind(class_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"class_id": class_id})))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentItemRequest {
    kind: String,
    slug: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAssignmentRequest {
    title: String,
    description: String,
    #[serde(with = "time::serde::rfc3339")]
    due_at: OffsetDateTime,
    completion_goal_percent: i16,
    items: Vec<AssignmentItemRequest>,
}

pub async fn create_assignment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
    Json(request): Json<CreateAssignmentRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ClassError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_instructor(state.pool(), class_id, user_id).await?;
    let unique_items: HashSet<_> = request
        .items
        .iter()
        .map(|item| (item.kind.as_str(), item.slug.as_str()))
        .collect();
    if request.title.trim().is_empty()
        || request.title.chars().count() > 120
        || request.description.chars().count() > 2000
        || request.due_at <= OffsetDateTime::now_utc()
        || !(1..=100).contains(&request.completion_goal_percent)
        || request.items.is_empty()
        || request.items.len() > 50
        || unique_items.len() != request.items.len()
    {
        return Err(ClassError::InvalidInput("과제 설정을 확인해 주세요"));
    }
    let mut transaction = state.pool().begin().await?;
    let assignment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO class_assignments (
            class_id, title_ko, description_ko, due_at, completion_goal_percent, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id
        "#,
    )
    .bind(class_id)
    .bind(request.title.trim())
    .bind(request.description.trim())
    .bind(request.due_at)
    .bind(request.completion_goal_percent)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    for (index, item) in request.items.iter().enumerate() {
        let inserted = match item.kind.as_str() {
            "problem" => {
                sqlx::query(
                    r#"
                    INSERT INTO class_assignment_items (assignment_id, item_kind, problem_id, position)
                    SELECT $1, 'problem', id, $2 FROM problems WHERE slug = $3 AND status = 'published'
                    "#,
                )
                .bind(assignment_id)
                .bind(index as i32 + 1)
                .bind(&item.slug)
                .execute(&mut *transaction)
                .await?
            }
            "activity" => {
                sqlx::query(
                    r#"
                    INSERT INTO class_assignment_items (assignment_id, item_kind, activity_id, position)
                    SELECT $1, 'activity', id, $2 FROM learning_activities WHERE slug = $3 AND status = 'published'
                    "#,
                )
                .bind(assignment_id)
                .bind(index as i32 + 1)
                .bind(&item.slug)
                .execute(&mut *transaction)
                .await?
            }
            _ => return Err(ClassError::InvalidInput("과제 항목 종류를 확인해 주세요")),
        };
        if inserted.rows_affected() != 1 {
            return Err(ClassError::InvalidInput(
                "공개된 학습 항목만 과제에 추가할 수 있습니다",
            ));
        }
    }
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'class.assignment.created', 'class_assignment', $2)",
    )
    .bind(user_id)
    .bind(assignment_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": assignment_id})),
    ))
}

#[derive(Debug, Serialize, FromRow)]
pub struct ClassView {
    id: Uuid,
    organization_id: Uuid,
    organization_name: String,
    name: String,
    status: String,
    viewer_role: String,
    learner_count: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AssignmentView {
    id: Uuid,
    title: String,
    description: String,
    #[serde(with = "time::serde::rfc3339")]
    published_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    due_at: OffsetDateTime,
    completion_goal_percent: i16,
    total_items: i64,
    completed_items: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AssignmentItemView {
    assignment_id: Uuid,
    kind: String,
    slug: String,
    title: String,
    position: i32,
    completed: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct ClassDetailResponse {
    class: ClassView,
    assignments: Vec<AssignmentView>,
    items: Vec<AssignmentItemView>,
}

async fn class_view(pool: &PgPool, class_id: Uuid, user_id: Uuid) -> Result<ClassView, ClassError> {
    sqlx::query_as::<_, ClassView>(
        r#"
        SELECT class.id, class.organization_id, organization.name AS organization_name,
               class.name, class.status,
               CASE WHEN organization_membership.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR')
                          OR class_membership.role = 'INSTRUCTOR'
                    THEN 'INSTRUCTOR' ELSE 'LEARNER' END AS viewer_role,
               (SELECT COUNT(*) FROM class_memberships member WHERE member.class_id = class.id AND member.role = 'LEARNER') AS learner_count
        FROM classes class
        JOIN organizations organization ON organization.id = class.organization_id
        LEFT JOIN class_memberships class_membership
          ON class_membership.class_id = class.id AND class_membership.user_id = $2
        LEFT JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = class.organization_id
         AND organization_membership.user_id = $2
        WHERE class.id = $1 AND class.status = 'active'
          AND (class_membership.user_id IS NOT NULL
               OR organization_membership.role IN ('OWNER', 'ADMIN', 'INSTRUCTOR'))
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ClassError::NotFound)
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
) -> Result<Json<ClassDetailResponse>, ClassError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let class = class_view(state.pool(), class_id, user_id).await?;
    let assignments = sqlx::query_as::<_, AssignmentView>(
        r#"
        SELECT assignment.id, assignment.title_ko AS title,
               assignment.description_ko AS description, assignment.published_at,
               assignment.due_at, assignment.completion_goal_percent,
               (SELECT COUNT(*) FROM class_assignment_items item WHERE item.assignment_id = assignment.id) AS total_items,
               (SELECT COUNT(*) FROM class_assignment_items item
                WHERE item.assignment_id = assignment.id AND (
                    (item.item_kind = 'problem' AND EXISTS (
                        SELECT 1 FROM submissions submission
                        WHERE submission.user_id = $2 AND submission.problem_id = item.problem_id
                          AND submission.run_kind = 'formal' AND submission.status = 'ACCEPTED'
                          AND submission.created_at >= assignment.published_at
                    ))
                    OR (item.item_kind = 'activity' AND EXISTS (
                        SELECT 1 FROM activity_attempts attempt
                        WHERE attempt.user_id = $2 AND attempt.activity_id = item.activity_id
                          AND attempt.passed AND attempt.created_at >= assignment.published_at
                    ))
                )) AS completed_items
        FROM class_assignments assignment
        WHERE assignment.class_id = $1 AND assignment.status = 'active'
        ORDER BY assignment.due_at, assignment.id
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let items = sqlx::query_as::<_, AssignmentItemView>(
        r#"
        SELECT item.assignment_id, item.item_kind AS kind,
               COALESCE(problem.slug, activity.slug) AS slug,
               COALESCE(problem.title_ko, activity.title_ko) AS title,
               item.position,
               CASE WHEN item.item_kind = 'problem' THEN problem_completion.completed_at IS NOT NULL
                    ELSE activity_completion.completed_at IS NOT NULL END AS completed,
               CASE WHEN item.item_kind = 'problem' THEN problem_completion.completed_at
                    ELSE activity_completion.completed_at END AS completed_at
        FROM class_assignment_items item
        JOIN class_assignments assignment ON assignment.id = item.assignment_id
        LEFT JOIN problems problem ON problem.id = item.problem_id
        LEFT JOIN learning_activities activity ON activity.id = item.activity_id
        LEFT JOIN LATERAL (
            SELECT MIN(submission.judged_at) AS completed_at FROM submissions submission
            WHERE submission.user_id = $2 AND submission.problem_id = item.problem_id
              AND submission.run_kind = 'formal' AND submission.status = 'ACCEPTED'
              AND submission.created_at >= assignment.published_at
        ) problem_completion ON true
        LEFT JOIN LATERAL (
            SELECT MIN(attempt.created_at) AS completed_at FROM activity_attempts attempt
            WHERE attempt.user_id = $2 AND attempt.activity_id = item.activity_id
              AND attempt.passed AND attempt.created_at >= assignment.published_at
        ) activity_completion ON true
        WHERE assignment.class_id = $1 AND assignment.status = 'active'
        ORDER BY assignment.due_at, item.position
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(ClassDetailResponse {
        class,
        assignments,
        items,
    }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct DashboardAssignment {
    id: Uuid,
    title: String,
    #[serde(with = "time::serde::rfc3339")]
    due_at: OffsetDateTime,
    goal_percent: i16,
    total_items: i64,
    learner_count: i64,
    completed_slots: i64,
    completion_percent: i64,
    goal_met: bool,
}

#[derive(Debug, Serialize, FromRow)]
pub struct LearnerProgress {
    user_id: Uuid,
    handle: String,
    display_name: String,
    total_items: i64,
    completed_items: i64,
    completion_percent: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct CommonError {
    code: String,
    label: String,
    occurrences: i64,
}

#[derive(Debug, Serialize)]
pub struct InstructorDashboardResponse {
    assignments: Vec<DashboardAssignment>,
    learners: Vec<LearnerProgress>,
    common_errors: Vec<CommonError>,
}

async fn instructor_dashboard_data(
    pool: &PgPool,
    class_id: Uuid,
) -> Result<InstructorDashboardResponse, ClassError> {
    let assignments = sqlx::query_as::<_, DashboardAssignment>(
        r#"
        SELECT assignment.id, assignment.title_ko AS title, assignment.due_at,
               assignment.completion_goal_percent AS goal_percent,
               (SELECT COUNT(*) FROM class_assignment_items item WHERE item.assignment_id = assignment.id) AS total_items,
               (SELECT COUNT(*) FROM class_memberships member WHERE member.class_id = assignment.class_id AND member.role = 'LEARNER') AS learner_count,
               (SELECT COUNT(*) FROM class_memberships member
                CROSS JOIN class_assignment_items item
                WHERE member.class_id = assignment.class_id AND member.role = 'LEARNER'
                  AND item.assignment_id = assignment.id AND (
                    (item.item_kind = 'problem' AND EXISTS (
                        SELECT 1 FROM submissions submission
                        WHERE submission.user_id = member.user_id AND submission.problem_id = item.problem_id
                          AND submission.run_kind = 'formal' AND submission.status = 'ACCEPTED'
                          AND submission.created_at >= assignment.published_at
                    ))
                    OR (item.item_kind = 'activity' AND EXISTS (
                        SELECT 1 FROM activity_attempts attempt
                        WHERE attempt.user_id = member.user_id AND attempt.activity_id = item.activity_id
                          AND attempt.passed AND attempt.created_at >= assignment.published_at
                    ))
                  )) AS completed_slots,
               0::bigint AS completion_percent, false AS goal_met
        FROM class_assignments assignment
        WHERE assignment.class_id = $1 AND assignment.status = 'active'
        ORDER BY assignment.due_at, assignment.id
        "#,
    )
    .bind(class_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|mut assignment| {
        let slots = assignment.total_items * assignment.learner_count;
        assignment.completion_percent = if slots == 0 {
            0
        } else {
            assignment.completed_slots * 100 / slots
        };
        assignment.goal_met = assignment.completion_percent >= i64::from(assignment.goal_percent);
        assignment
    })
    .collect();
    let learners = sqlx::query_as::<_, LearnerProgress>(
        r#"
        SELECT user_account.id AS user_id, user_account.handle, user_account.display_name,
               (SELECT COUNT(*) FROM class_assignments assignment
                JOIN class_assignment_items item ON item.assignment_id = assignment.id
                WHERE assignment.class_id = $1 AND assignment.status = 'active') AS total_items,
               (SELECT COUNT(*) FROM class_assignments assignment
                JOIN class_assignment_items item ON item.assignment_id = assignment.id
                WHERE assignment.class_id = $1 AND assignment.status = 'active' AND (
                    (item.item_kind = 'problem' AND EXISTS (
                        SELECT 1 FROM submissions submission
                        WHERE submission.user_id = member.user_id AND submission.problem_id = item.problem_id
                          AND submission.run_kind = 'formal' AND submission.status = 'ACCEPTED'
                          AND submission.created_at >= assignment.published_at
                    ))
                    OR (item.item_kind = 'activity' AND EXISTS (
                        SELECT 1 FROM activity_attempts attempt
                        WHERE attempt.user_id = member.user_id AND attempt.activity_id = item.activity_id
                          AND attempt.passed AND attempt.created_at >= assignment.published_at
                    ))
                )) AS completed_items,
               0::bigint AS completion_percent
        FROM class_memberships member
        JOIN users user_account ON user_account.id = member.user_id
        WHERE member.class_id = $1 AND member.role = 'LEARNER'
        ORDER BY user_account.display_name, user_account.id
        "#,
    )
    .bind(class_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|mut learner| {
        learner.completion_percent = if learner.total_items == 0 {
            0
        } else {
            learner.completed_items * 100 / learner.total_items
        };
        learner
    })
    .collect();
    let common_errors = sqlx::query_as::<_, CommonError>(
        r#"
        WITH problem_error AS (
            SELECT DISTINCT submission.id, submission.status AS code
            FROM class_assignments assignment
            JOIN class_assignment_items item ON item.assignment_id = assignment.id AND item.item_kind = 'problem'
            JOIN class_memberships member ON member.class_id = assignment.class_id AND member.role = 'LEARNER'
            JOIN submissions submission ON submission.user_id = member.user_id
             AND submission.problem_id = item.problem_id AND submission.run_kind = 'formal'
             AND submission.created_at >= assignment.published_at
            WHERE assignment.class_id = $1 AND submission.status NOT IN (
                'QUEUED', 'COMPILING', 'RUNNING', 'ACCEPTED', 'CANCELLED'
            )
        ), activity_error AS (
            SELECT DISTINCT attempt.id, 'ACTIVITY_FAILED'::text AS code
            FROM class_assignments assignment
            JOIN class_assignment_items item ON item.assignment_id = assignment.id AND item.item_kind = 'activity'
            JOIN class_memberships member ON member.class_id = assignment.class_id AND member.role = 'LEARNER'
            JOIN activity_attempts attempt ON attempt.user_id = member.user_id
             AND attempt.activity_id = item.activity_id AND attempt.created_at >= assignment.published_at
            WHERE assignment.class_id = $1 AND NOT attempt.passed
        ), errors AS (
            SELECT code FROM problem_error UNION ALL SELECT code FROM activity_error
        )
        SELECT code,
               CASE code WHEN 'WRONG_ANSWER' THEN '오답' WHEN 'COMPILE_ERROR' THEN '컴파일 오류'
                    WHEN 'RUNTIME_ERROR' THEN '실행 오류' WHEN 'TIME_LIMIT_EXCEEDED' THEN '시간 초과'
                    WHEN 'MEMORY_LIMIT_EXCEEDED' THEN '메모리 초과'
                    WHEN 'OUTPUT_LIMIT_EXCEEDED' THEN '출력 초과'
                    WHEN 'PARTIAL_ACCEPTED' THEN '부분 정답' WHEN 'ACTIVITY_FAILED' THEN '활동 기준 미달'
                    ELSE '기타 판정 오류' END AS label,
               COUNT(*)::bigint AS occurrences
        FROM errors GROUP BY code ORDER BY occurrences DESC, code
        "#,
    )
    .bind(class_id)
    .fetch_all(pool)
    .await?;
    Ok(InstructorDashboardResponse {
        assignments,
        learners,
        common_errors,
    })
}

pub async fn reward_completed_assignments(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    let completed: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT assignment.id
        FROM class_assignments assignment
        JOIN class_memberships member ON member.class_id = assignment.class_id
         AND member.user_id = $1 AND member.role = 'LEARNER'
        WHERE assignment.status = 'active'
          AND EXISTS (SELECT 1 FROM class_assignment_items item WHERE item.assignment_id = assignment.id)
          AND NOT EXISTS (
              SELECT 1 FROM xp_events reward
              WHERE reward.user_id = $1
                AND reward.reward_key = 'class-assignment:' || assignment.id::text
          )
          AND NOT EXISTS (
              SELECT 1 FROM class_assignment_items item
              WHERE item.assignment_id = assignment.id AND NOT (
                  (item.item_kind = 'problem' AND EXISTS (
                      SELECT 1 FROM submissions submission
                      WHERE submission.user_id = $1 AND submission.problem_id = item.problem_id
                        AND submission.run_kind = 'formal' AND submission.status = 'ACCEPTED'
                        AND submission.created_at >= assignment.published_at
                  ))
                  OR (item.item_kind = 'activity' AND EXISTS (
                      SELECT 1 FROM activity_attempts attempt
                      WHERE attempt.user_id = $1 AND attempt.activity_id = item.activity_id
                        AND attempt.passed AND attempt.created_at >= assignment.published_at
                  ))
              )
          )
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await?;
    for assignment_id in completed {
        crate::gamification::apply_reward(
            transaction,
            crate::gamification::RewardSpec {
                event_id: Uuid::now_v7(),
                user_id,
                reward_key: format!("class-assignment:{assignment_id}"),
                source_kind: "course",
                source_id: Some(assignment_id),
                xp: 50,
                reason_ko: "학급 과제 완료",
                axis: "instructor_course",
                mastery_points: 30,
                mastery_class: "completed",
            },
        )
        .await?;
    }
    Ok(())
}

pub async fn instructor_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
) -> Result<Json<InstructorDashboardResponse>, ClassError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_instructor(state.pool(), class_id, user_id).await?;
    Ok(Json(
        instructor_dashboard_data(state.pool(), class_id).await?,
    ))
}

fn csv_cell(value: &str) -> String {
    let normalized = value.replace(['\r', '\n'], " ");
    let safe = if normalized
        .trim_start()
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '=' | '+' | '-' | '@'))
    {
        format!("'{normalized}")
    } else {
        normalized
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

pub async fn export_csv(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
) -> Result<Response, ClassError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_instructor(state.pool(), class_id, user_id).await?;
    let dashboard = instructor_dashboard_data(state.pool(), class_id).await?;
    let mut csv = String::from("\u{feff}이름,핸들,완료 항목,전체 항목,완료율\r\n");
    for learner in dashboard.learners {
        csv.push_str(&format!(
            "{},{},{},{},{}%\r\n",
            csv_cell(&learner.display_name),
            csv_cell(&learner.handle),
            learner.completed_items,
            learner.total_items,
            learner.completion_percent
        ));
    }
    let mut response = Response::new(Body::from(csv));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=alpha-class-progress.csv"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
