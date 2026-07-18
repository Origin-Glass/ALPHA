use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum AdminError {
    Auth(AuthError),
    InvalidInput(&'static str),
    RoleRequired,
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::RoleRequired => (
                StatusCode::FORBIDDEN,
                "admin_role_required",
                "요청한 관리 권한이 필요합니다",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "admin_resource_not_found",
                "관리 대상을 찾을 수 없습니다",
            ),
            Self::Database(error) => {
                tracing::error!(%error, "관리 데이터베이스 처리 실패");
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

impl From<AuthError> for AdminError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for AdminError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn has_role(state: &AppState, user_id: Uuid, roles: &[&str]) -> Result<bool, AdminError> {
    let roles: Vec<String> = roles.iter().map(|role| (*role).to_owned()).collect();
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role = ANY($2::text[]))",
    )
    .bind(user_id)
    .bind(roles)
    .fetch_one(state.pool())
    .await?)
}

async fn require_role(state: &AppState, user_id: Uuid, roles: &[&str]) -> Result<(), AdminError> {
    if !has_role(state, user_id, roles).await? {
        return Err(AdminError::RoleRequired);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemTagRequest {
    tag: String,
    label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaseRequest {
    input: String,
    expected_output: String,
    visibility: String,
    score_weight: i32,
    group_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemContentRequest {
    title: String,
    statement: String,
    difficulty: i16,
    learning_axis: String,
    status: String,
    time_limit_ms: i32,
    memory_limit_mb: i32,
    checker_kind: String,
    float_tolerance: Option<f64>,
    tags: Vec<ProblemTagRequest>,
    test_cases: Vec<TestCaseRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProblemRequest {
    slug: String,
    #[serde(flatten)]
    content: ProblemContentRequest,
}

fn valid_content(request: &ProblemContentRequest) -> bool {
    let tags: HashSet<_> = request.tags.iter().map(|tag| tag.tag.as_str()).collect();
    let total_case_bytes: usize = request
        .test_cases
        .iter()
        .map(|test| test.input.len() + test.expected_output.len())
        .sum();
    !request.title.trim().is_empty()
        && request.title.chars().count() <= 120
        && !request.statement.trim().is_empty()
        && request.statement.chars().count() <= 50_000
        && (0..=30).contains(&request.difficulty)
        && matches!(
            request.learning_axis.as_str(),
            "algorithmic_reasoning" | "code_literacy" | "docs_learning" | "independent_coding"
        )
        && request.status == "draft"
        && (100..=30_000).contains(&request.time_limit_ms)
        && (16..=2048).contains(&request.memory_limit_mb)
        && matches!(
            request.checker_kind.as_str(),
            "exact" | "whitespace" | "float"
        )
        && ((request.checker_kind == "float")
            == request
                .float_tolerance
                .is_some_and(|tolerance| tolerance > 0.0 && tolerance <= 0.1))
        && request.tags.len() <= 30
        && tags.len() == request.tags.len()
        && request.tags.iter().all(|tag| {
            !tag.label.trim().is_empty()
                && tag.label.chars().count() <= 40
                && !tag.tag.is_empty()
                && tag.tag.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
        && !request.test_cases.is_empty()
        && request.test_cases.len() <= 200
        && total_case_bytes <= 2_000_000
        && request.test_cases.iter().all(|test| {
            matches!(test.visibility.as_str(), "sample" | "hidden")
                && (1..=1000).contains(&test.score_weight)
                && !test.group_key.is_empty()
                && test.group_key.len() <= 32
                && test
                    .group_key
                    .chars()
                    .enumerate()
                    .all(|(index, character)| {
                        character.is_ascii_lowercase()
                            || character.is_ascii_digit()
                            || (index > 0 && matches!(character, '_' | '-'))
                    })
        })
        && request
            .test_cases
            .iter()
            .any(|test| test.visibility == "sample")
        && request
            .test_cases
            .iter()
            .any(|test| test.visibility == "hidden")
}

fn content_hash(request: &ProblemContentRequest) -> Vec<u8> {
    fn add(hasher: &mut Sha256, value: &[u8]) {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    let mut hasher = Sha256::new();
    add(&mut hasher, request.title.trim().as_bytes());
    add(&mut hasher, request.statement.trim().as_bytes());
    add(&mut hasher, &request.difficulty.to_be_bytes());
    add(&mut hasher, request.learning_axis.as_bytes());
    add(&mut hasher, request.status.as_bytes());
    add(&mut hasher, &request.time_limit_ms.to_be_bytes());
    add(&mut hasher, &request.memory_limit_mb.to_be_bytes());
    add(&mut hasher, request.checker_kind.as_bytes());
    add(
        &mut hasher,
        request
            .float_tolerance
            .map(|value| value.to_bits().to_be_bytes().to_vec())
            .as_deref()
            .unwrap_or_default(),
    );
    for tag in &request.tags {
        add(&mut hasher, tag.tag.as_bytes());
        add(&mut hasher, tag.label.trim().as_bytes());
    }
    for test in &request.test_cases {
        add(&mut hasher, test.input.as_bytes());
        add(&mut hasher, test.expected_output.as_bytes());
        add(&mut hasher, test.visibility.as_bytes());
        add(&mut hasher, &test.score_weight.to_be_bytes());
        add(&mut hasher, test.group_key.as_bytes());
    }
    hasher.finalize().to_vec()
}

async fn replace_problem_content(
    transaction: &mut Transaction<'_, Postgres>,
    problem_id: Uuid,
    user_id: Uuid,
    request: &ProblemContentRequest,
) -> Result<(Uuid, i32), AdminError> {
    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM problem_revisions WHERE problem_id = $1",
    )
    .bind(problem_id)
    .fetch_one(&mut **transaction)
    .await?;
    let revision_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO problem_revisions (
            problem_id, version, statement_ko, time_limit_ms, memory_limit_mb,
            checker_kind, float_tolerance, content_hash, authored_by
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id
        "#,
    )
    .bind(problem_id)
    .bind(version)
    .bind(request.statement.trim())
    .bind(request.time_limit_ms)
    .bind(request.memory_limit_mb)
    .bind(&request.checker_kind)
    .bind(request.float_tolerance)
    .bind(content_hash(request))
    .bind(user_id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM problem_tags WHERE problem_id = $1")
        .bind(problem_id)
        .execute(&mut **transaction)
        .await?;
    for tag in &request.tags {
        sqlx::query("INSERT INTO problem_tags (problem_id, tag, label_ko) VALUES ($1, $2, $3)")
            .bind(problem_id)
            .bind(&tag.tag)
            .bind(tag.label.trim())
            .execute(&mut **transaction)
            .await?;
    }
    for (index, test) in request.test_cases.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO problem_test_cases (
                problem_id, problem_revision_id, ordinal, input, expected_output,
                visibility, score_weight, group_key
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(problem_id)
        .bind(revision_id)
        .bind(index as i32 + 1)
        .bind(&test.input)
        .bind(&test.expected_output)
        .bind(&test.visibility)
        .bind(test.score_weight)
        .bind(&test.group_key)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        r#"
        UPDATE problems SET title_ko = $2, statement_ko = $3, difficulty = $4,
            learning_axis = $5, status = $6, time_limit_ms = $7, memory_limit_mb = $8,
            checker_kind = $9, float_tolerance = $10, current_revision_id = $11,
            published_at = CASE WHEN $6 = 'published' THEN COALESCE(published_at, now()) ELSE NULL END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(problem_id)
    .bind(request.title.trim())
    .bind(request.statement.trim())
    .bind(request.difficulty)
    .bind(&request.learning_axis)
    .bind(&request.status)
    .bind(request.time_limit_ms)
    .bind(request.memory_limit_mb)
    .bind(&request.checker_kind)
    .bind(request.float_tolerance)
    .bind(revision_id)
    .execute(&mut **transaction)
    .await?;
    Ok((revision_id, version))
}

pub async fn create_problem(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateProblemRequest>,
) -> Result<(StatusCode, Json<Value>), AdminError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !crate::governance::has_capability(state.pool(), user_id, "content.create").await? {
        return Err(AdminError::RoleRequired);
    }
    if request.slug.is_empty()
        || request.slug.len() > 80
        || !request.slug.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || (index > 0 && character == '-')
        })
        || request.slug.ends_with('-')
        || !valid_content(&request.content)
    {
        return Err(AdminError::InvalidInput(
            "문제 설정과 테스트를 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let problem_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO problems (
            slug, title_ko, statement_ko, difficulty, learning_axis, status,
            time_limit_ms, memory_limit_mb, checker_kind, float_tolerance, created_by
        ) VALUES ($1,$2,$3,$4,$5,'draft',$6,$7,$8,$9,$10) RETURNING id
        "#,
    )
    .bind(&request.slug)
    .bind(request.content.title.trim())
    .bind(request.content.statement.trim())
    .bind(request.content.difficulty)
    .bind(&request.content.learning_axis)
    .bind(request.content.time_limit_ms)
    .bind(request.content.memory_limit_mb)
    .bind(&request.content.checker_kind)
    .bind(request.content.float_tolerance)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    let (revision_id, version) =
        replace_problem_content(&mut transaction, problem_id, user_id, &request.content).await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'problem.created', 'problem', $2, jsonb_build_object('revision', $3::integer))",
    )
    .bind(user_id)
    .bind(problem_id.to_string())
    .bind(version)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(
            serde_json::json!({"id": problem_id, "slug": request.slug, "revision_id": revision_id, "version": version}),
        ),
    ))
}

pub async fn revise_problem(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<ProblemContentRequest>,
) -> Result<Json<Value>, AdminError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !crate::governance::has_capability(state.pool(), user_id, "content.create").await? {
        return Err(AdminError::RoleRequired);
    }
    if !valid_content(&request) {
        return Err(AdminError::InvalidInput(
            "문제 설정과 테스트를 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let problem_id: Uuid = sqlx::query_scalar(
        r#"
        SELECT id FROM problems
        WHERE slug = $1 AND source_kind = 'original'
          AND (
              created_by = $2 OR EXISTS (
                  SELECT 1 FROM user_roles
                  WHERE user_id = $2 AND role = 'ADMIN'
              )
          )
        FOR UPDATE
        "#,
    )
    .bind(&slug)
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AdminError::NotFound)?;
    let (revision_id, version) =
        replace_problem_content(&mut transaction, problem_id, user_id, &request).await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'problem.revised', 'problem', $2, jsonb_build_object('revision', $3::integer))",
    )
    .bind(user_id)
    .bind(problem_id.to_string())
    .bind(version)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"revision_id": revision_id, "version": version}),
    ))
}

#[derive(Debug, Serialize, FromRow)]
pub struct WorkerView {
    worker_id: String,
    protocol_version: i16,
    image_reference: String,
    status: String,
    health: String,
    current_job_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    last_heartbeat_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkerListResponse {
    items: Vec<WorkerView>,
}

pub async fn workers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WorkerListResponse>, AdminError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_role(&state, user_id, &["ADMIN"]).await?;
    let items = sqlx::query_as::<_, WorkerView>(
        r#"
        SELECT worker_id, protocol_version, image_reference, status,
               CASE WHEN last_heartbeat_at < now() - interval '30 seconds' THEN 'stale'
                    ELSE 'healthy' END AS health,
               current_job_id, last_heartbeat_at, started_at
        FROM judge_workers ORDER BY last_heartbeat_at DESC, worker_id
        "#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(WorkerListResponse { items }))
}

pub async fn operations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::observability::OperationalSnapshot>, AdminError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_role(&state, user_id, &["ADMIN"]).await?;
    Ok(Json(
        crate::observability::operational_snapshot(state.pool()).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    limit: Option<u16>,
    action: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AuditView {
    id: Uuid,
    actor_handle: Option<String>,
    action: String,
    target_type: String,
    target_id: Option<String>,
    metadata: sqlx::types::Json<Value>,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AuditListResponse {
    items: Vec<AuditView>,
}

pub async fn audit_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AuditListResponse>, AdminError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_role(&state, user_id, &["ADMIN"]).await?;
    if query
        .action
        .as_ref()
        .is_some_and(|action| action.len() > 120)
    {
        return Err(AdminError::InvalidInput("감사 필터를 확인해 주세요"));
    }
    let items = sqlx::query_as::<_, AuditView>(
        r#"
        SELECT event.id, actor.handle AS actor_handle, event.action, event.target_type,
               event.target_id, event.metadata, event.occurred_at
        FROM audit_events event
        LEFT JOIN users actor ON actor.id = event.actor_user_id
        WHERE ($1::text IS NULL OR event.action = $1)
        ORDER BY event.occurred_at DESC, event.id DESC LIMIT $2
        "#,
    )
    .bind(query.action)
    .bind(i64::from(query.limit.unwrap_or(100).clamp(1, 200)))
    .fetch_all(state.pool())
    .await?;
    Ok(Json(AuditListResponse { items }))
}
