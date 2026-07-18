use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum CommunityError {
    Auth(AuthError),
    InvalidInput(&'static str),
    RoleRequired,
    DuplicateReport,
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for CommunityError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::RoleRequired => (
                StatusCode::FORBIDDEN,
                "moderator_required",
                "운영자 권한이 필요합니다",
            ),
            Self::DuplicateReport => (
                StatusCode::CONFLICT,
                "report_already_open",
                "이미 검토 중인 신고가 있습니다",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "community_content_not_found",
                "게시물을 찾을 수 없습니다",
            ),
            Self::Database(error) => {
                tracing::error!(%error, "커뮤니티 데이터베이스 처리 실패");
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

impl From<AuthError> for CommunityError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for CommunityError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn require_terms(state: &AppState, user_id: Uuid) -> Result<(), CommunityError> {
    let accepted: bool = sqlx::query_scalar(
        "SELECT terms_accepted_at IS NOT NULL FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    if !accepted {
        return Err(CommunityError::InvalidInput(
            "커뮤니티 참여 전에 이용약관에 동의해 주세요",
        ));
    }
    Ok(())
}

async fn has_role(state: &AppState, user_id: Uuid, roles: &[&str]) -> Result<bool, CommunityError> {
    let roles: Vec<String> = roles.iter().map(|role| (*role).to_owned()).collect();
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role = ANY($2::text[]))",
    )
    .bind(user_id)
    .bind(roles)
    .fetch_one(state.pool())
    .await?)
}

async fn require_moderator(state: &AppState, user_id: Uuid) -> Result<(), CommunityError> {
    if !has_role(state, user_id, &["MODERATOR", "ADMIN"]).await? {
        return Err(CommunityError::RoleRequired);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CommunityListQuery {
    kind: Option<String>,
    problem: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct PostListItem {
    id: Uuid,
    kind: String,
    title: String,
    body_preview: String,
    status: String,
    pinned: bool,
    author_handle: String,
    author_name: String,
    problem_slug: Option<String>,
    problem_title: Option<String>,
    answer_count: i64,
    has_accepted_answer: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct CommunityListResponse {
    items: Vec<PostListItem>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<CommunityListQuery>,
) -> Result<Json<CommunityListResponse>, CommunityError> {
    if query
        .kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "notice" | "question" | "discussion"))
    {
        return Err(CommunityError::InvalidInput("게시물 종류를 확인해 주세요"));
    }
    let items = sqlx::query_as::<_, PostListItem>(
        r#"
        SELECT post.id, post.kind, post.title_ko AS title,
               left(post.body_ko, 220) AS body_preview, post.status, post.pinned,
               author.handle AS author_handle, author.display_name AS author_name,
               problem.slug AS problem_slug, problem.title_ko AS problem_title,
               (SELECT COUNT(*) FROM community_answers answer WHERE answer.post_id = post.id AND answer.status = 'published') AS answer_count,
               EXISTS(SELECT 1 FROM community_answers answer WHERE answer.post_id = post.id AND answer.accepted_at IS NOT NULL AND answer.status = 'published') AS has_accepted_answer,
               post.created_at
        FROM community_posts post
        JOIN users author ON author.id = post.author_id
        LEFT JOIN problems problem ON problem.id = post.problem_id
        WHERE post.status IN ('published', 'locked')
          AND ($1::text IS NULL OR post.kind = $1)
          AND ($2::text IS NULL OR problem.slug = $2)
        ORDER BY post.pinned DESC, post.created_at DESC, post.id DESC
        LIMIT 100
        "#,
    )
    .bind(query.kind)
    .bind(query.problem)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(CommunityListResponse { items }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct PostDetail {
    id: Uuid,
    author_id: Uuid,
    kind: String,
    title: String,
    body: String,
    status: String,
    pinned: bool,
    author_handle: String,
    author_name: String,
    problem_slug: Option<String>,
    problem_title: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AnswerView {
    id: Uuid,
    author_id: Uuid,
    body: String,
    author_handle: String,
    author_name: String,
    accepted: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct CommunityDetailResponse {
    post: PostDetail,
    answers: Vec<AnswerView>,
    viewer_is_author: bool,
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(post_id): Path<Uuid>,
) -> Result<Json<CommunityDetailResponse>, CommunityError> {
    let post = sqlx::query_as::<_, PostDetail>(
        r#"
        SELECT post.id, post.author_id, post.kind, post.title_ko AS title,
               post.body_ko AS body, post.status, post.pinned,
               author.handle AS author_handle, author.display_name AS author_name,
               problem.slug AS problem_slug, problem.title_ko AS problem_title,
               post.created_at, post.updated_at
        FROM community_posts post
        JOIN users author ON author.id = post.author_id
        LEFT JOIN problems problem ON problem.id = post.problem_id
        WHERE post.id = $1 AND post.status IN ('published', 'locked')
        "#,
    )
    .bind(post_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(CommunityError::NotFound)?;
    let answers = sqlx::query_as::<_, AnswerView>(
        r#"
        SELECT answer.id, answer.author_id, answer.body_ko AS body,
               author.handle AS author_handle, author.display_name AS author_name,
               answer.accepted_at IS NOT NULL AS accepted, answer.created_at
        FROM community_answers answer
        JOIN users author ON author.id = answer.author_id
        WHERE answer.post_id = $1 AND answer.status = 'published'
        ORDER BY (answer.accepted_at IS NOT NULL) DESC, answer.created_at, answer.id
        "#,
    )
    .bind(post_id)
    .fetch_all(state.pool())
    .await?;
    let viewer_is_author = crate::auth::authenticated_user_id(&state, &headers)
        .await
        .is_ok_and(|user_id| user_id == post.author_id);
    Ok(Json(CommunityDetailResponse {
        post,
        answers,
        viewer_is_author,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePostRequest {
    kind: String,
    title: String,
    body: String,
    problem_slug: Option<String>,
}

fn valid_content(title: &str, body: &str) -> bool {
    !title.trim().is_empty()
        && title.chars().count() <= 160
        && !body.trim().is_empty()
        && body.chars().count() <= 10_000
        && !title.chars().any(char::is_control)
        && !body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

pub async fn create_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreatePostRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    if !matches!(request.kind.as_str(), "question" | "discussion")
        || !valid_content(&request.title, &request.body)
    {
        return Err(CommunityError::InvalidInput("게시물 내용을 확인해 주세요"));
    }
    let problem_id: Option<Uuid> = if let Some(problem_slug) = &request.problem_slug {
        Some(
            sqlx::query_scalar("SELECT id FROM problems WHERE slug = $1 AND status = 'published'")
                .bind(problem_slug)
                .fetch_optional(state.pool())
                .await?
                .ok_or(CommunityError::InvalidInput("공개된 문제를 선택해 주세요"))?,
        )
    } else {
        None
    };
    let post_id: Uuid = sqlx::query_scalar(
        "INSERT INTO community_posts (author_id, problem_id, kind, title_ko, body_ko) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(user_id)
    .bind(problem_id)
    .bind(&request.kind)
    .bind(request.title.trim())
    .bind(request.body.trim())
    .fetch_one(state.pool())
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": post_id})),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAnswerRequest {
    body: String,
}

pub async fn create_answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(post_id): Path<Uuid>,
    Json(request): Json<CreateAnswerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    if request.body.trim().is_empty()
        || request.body.chars().count() > 10_000
        || request
            .body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(CommunityError::InvalidInput("답변 내용을 확인해 주세요"));
    }
    let answer_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO community_answers (post_id, author_id, body_ko)
        SELECT id, $2, $3 FROM community_posts
        WHERE id = $1 AND kind IN ('question', 'discussion') AND status = 'published'
        RETURNING id
        "#,
    )
    .bind(post_id)
    .bind(user_id)
    .bind(request.body.trim())
    .fetch_optional(state.pool())
    .await?
    .ok_or(CommunityError::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": answer_id})),
    ))
}

pub async fn accept_answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((post_id, answer_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let owns_question: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM community_posts WHERE id = $1 AND author_id = $2 AND kind = 'question' AND status IN ('published', 'locked'))",
    )
    .bind(post_id)
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !owns_question {
        return Err(CommunityError::NotFound);
    }
    let mut transaction = state.pool().begin().await?;
    sqlx::query(
        "UPDATE community_answers SET accepted_by = NULL, accepted_at = NULL WHERE post_id = $1 AND accepted_at IS NOT NULL",
    )
    .bind(post_id)
    .execute(&mut *transaction)
    .await?;
    let updated = sqlx::query(
        "UPDATE community_answers SET accepted_by = $3, accepted_at = now() WHERE id = $2 AND post_id = $1 AND status = 'published'",
    )
    .bind(post_id)
    .bind(answer_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(CommunityError::NotFound);
    }
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'community.answer.accepted', 'community_answer', $2)",
    )
    .bind(user_id)
    .bind(answer_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateReportRequest {
    target_type: String,
    target_id: Uuid,
    reason: String,
    detail: String,
}

pub async fn create_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateReportRequest>,
) -> Result<StatusCode, CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    if !matches!(request.target_type.as_str(), "post" | "answer")
        || !matches!(
            request.reason.as_str(),
            "spam" | "abuse" | "solution_leak" | "privacy" | "other"
        )
        || request.detail.chars().count() > 2000
    {
        return Err(CommunityError::InvalidInput("신고 내용을 확인해 주세요"));
    }
    let result = match request.target_type.as_str() {
        "post" => {
            sqlx::query(
                r#"
                INSERT INTO community_reports (reporter_id, target_type, post_id, reason, detail_ko)
                SELECT $1, 'post', id, $3, $4 FROM community_posts
                WHERE id = $2 AND status IN ('published', 'locked')
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(request.target_id)
            .bind(&request.reason)
            .bind(request.detail.trim())
            .execute(state.pool())
            .await?
        }
        _ => {
            sqlx::query(
                r#"
                INSERT INTO community_reports (reporter_id, target_type, answer_id, reason, detail_ko)
                SELECT $1, 'answer', id, $3, $4 FROM community_answers
                WHERE id = $2 AND status = 'published'
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(request.target_id)
            .bind(&request.reason)
            .bind(request.detail.trim())
            .execute(state.pool())
            .await?
        }
    };
    if result.rows_affected() != 1 {
        let target_exists: bool = match request.target_type.as_str() {
            "post" => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM community_posts WHERE id = $1 AND status IN ('published', 'locked'))",
            )
            .bind(request.target_id)
            .fetch_one(state.pool())
            .await?,
            _ => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM community_answers WHERE id = $1 AND status = 'published')",
            )
            .bind(request.target_id)
            .fetch_one(state.pool())
            .await?,
        };
        return Err(if target_exists {
            CommunityError::DuplicateReport
        } else {
            CommunityError::NotFound
        });
    }
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Serialize, FromRow)]
pub struct ReportView {
    id: Uuid,
    target_type: String,
    target_id: Uuid,
    post_id: Uuid,
    reason: String,
    detail: String,
    reporter_handle: String,
    target_preview: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ReportQueueResponse {
    items: Vec<ReportView>,
}

pub async fn report_queue(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ReportQueueResponse>, CommunityError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    require_moderator(&state, user_id).await?;
    let items = sqlx::query_as::<_, ReportView>(
        r#"
        SELECT report.id, report.target_type,
               COALESCE(report.post_id, report.answer_id) AS target_id,
               COALESCE(report.post_id, answer.post_id) AS post_id,
               report.reason, report.detail_ko AS detail,
               reporter.handle AS reporter_handle,
               left(COALESCE(post.body_ko, answer.body_ko), 240) AS target_preview,
               report.created_at
        FROM community_reports report
        JOIN users reporter ON reporter.id = report.reporter_id
        LEFT JOIN community_posts post ON post.id = report.post_id
        LEFT JOIN community_answers answer ON answer.id = report.answer_id
        WHERE report.status = 'open'
        ORDER BY report.created_at, report.id
        LIMIT 200
        "#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(ReportQueueResponse { items }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveReportRequest {
    action: String,
    note: String,
}

pub async fn resolve_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(report_id): Path<Uuid>,
    Json(request): Json<ResolveReportRequest>,
) -> Result<StatusCode, CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_moderator(&state, user_id).await?;
    if !matches!(request.action.as_str(), "hide" | "dismiss")
        || !(3..=2000).contains(&request.note.trim().chars().count())
    {
        return Err(CommunityError::InvalidInput("처리 내용을 확인해 주세요"));
    }
    let mut transaction = state.pool().begin().await?;
    let report: Option<(String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT target_type, post_id, answer_id FROM community_reports WHERE id = $1 AND status = 'open' FOR UPDATE",
    )
    .bind(report_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (target_type, post_id, answer_id) = report.ok_or(CommunityError::NotFound)?;
    if request.action == "hide" {
        if target_type == "post" {
            sqlx::query(
                "UPDATE community_posts SET status = 'hidden', updated_at = now() WHERE id = $1",
            )
            .bind(post_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query("UPDATE community_answers SET status = 'hidden', updated_at = now(), accepted_by = NULL, accepted_at = NULL WHERE id = $1")
                .bind(answer_id)
                .execute(&mut *transaction)
                .await?;
        }
    }
    let status = if request.action == "hide" {
        "resolved"
    } else {
        "dismissed"
    };
    sqlx::query(
        "UPDATE community_reports SET status = $2, handled_by = $3, handled_at = now(), resolution_note_ko = $4 WHERE id = $1",
    )
    .bind(report_id)
    .bind(status)
    .bind(user_id)
    .bind(request.note.trim())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'community_report', $3, jsonb_build_object('target_type', $4::text))",
    )
    .bind(user_id)
    .bind(if request.action == "hide" { "community.report.hidden" } else { "community.report.dismissed" })
    .bind(report_id.to_string())
    .bind(target_type)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateNoticeRequest {
    title: String,
    body: String,
    pinned: bool,
}

pub async fn create_notice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateNoticeRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), CommunityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !has_role(&state, user_id, &["ADMIN"]).await? {
        return Err(CommunityError::RoleRequired);
    }
    if !valid_content(&request.title, &request.body) {
        return Err(CommunityError::InvalidInput("공지 내용을 확인해 주세요"));
    }
    let mut transaction = state.pool().begin().await?;
    let post_id: Uuid = sqlx::query_scalar(
        "INSERT INTO community_posts (author_id, kind, title_ko, body_ko, pinned) VALUES ($1, 'notice', $2, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(request.title.trim())
    .bind(request.body.trim())
    .bind(request.pinned)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'community.notice.created', 'community_post', $2)",
    )
    .bind(user_id)
    .bind(post_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": post_id})),
    ))
}
