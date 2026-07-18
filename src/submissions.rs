use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Sse, sse::Event, sse::KeepAlive},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSubmissionRequest {
    problem_slug: String,
    language: String,
    source: String,
    idempotency_key: Uuid,
    contest_slug: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    problem_slug: String,
    language: String,
    source: String,
    mode: String,
    custom_input: Option<String>,
    idempotency_key: Uuid,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SubmissionView {
    id: Uuid,
    problem_slug: String,
    problem_title: String,
    language: String,
    source: String,
    status: String,
    score: Option<i16>,
    compile_output: Option<String>,
    run_kind: String,
    run_output: Option<String>,
    created_at: OffsetDateTime,
    judged_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SubmissionListItem {
    id: Uuid,
    problem_slug: String,
    problem_title: String,
    language: String,
    status: String,
    score: Option<i16>,
    run_kind: String,
    created_at: OffsetDateTime,
    judged_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmissionListQuery {
    problem_slug: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDraftRequest {
    language: String,
    source: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejudgeRequest {
    reason: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DraftView {
    problem_slug: String,
    language: String,
    source: String,
    revision_count: i32,
    updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub enum SubmissionError {
    Auth(AuthError),
    InvalidInput(&'static str),
    TermsRequired,
    NotFound,
    RateLimited,
    RoleForbidden,
    ContestForbidden,
    Database(sqlx::Error),
}

impl IntoResponse for SubmissionError {
    fn into_response(self) -> Response {
        match self {
            Self::Auth(error) => error.into_response(),
            Self::InvalidInput(message) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"code": "invalid_input", "message": message}})),
            )
                .into_response(),
            Self::TermsRequired => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": {"code": "terms_required", "message": "제출 전에 이용약관에 동의해 주세요"}})),
            )
                .into_response(),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"code": "submission_or_problem_not_found", "message": "문제 또는 제출을 찾을 수 없습니다"}})),
            )
                .into_response(),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": {"code": "submission_rate_limited", "message": "제출이 너무 빠릅니다. 잠시 후 다시 시도해 주세요"}})),
            )
                .into_response(),
            Self::RoleForbidden => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": {"code": "judge_admin_required", "message": "문제 출제자 또는 관리자 권한이 필요합니다"}})),
            )
                .into_response(),
            Self::ContestForbidden => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": {"code": "contest_submission_forbidden", "message": "진행 중인 대회에 참가 등록한 뒤 해당 문제를 제출해 주세요"}})),
            )
                .into_response(),
            Self::Database(error) => {
                tracing::error!(%error, "제출 데이터베이스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"code": "internal_error", "message": "요청을 처리하지 못했습니다"}})),
                )
                    .into_response()
            }
        }
    }
}

impl From<AuthError> for SubmissionError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for SubmissionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

fn valid_language(language: &str) -> bool {
    matches!(language, "cpp20" | "python3" | "java21")
}

fn valid_source(source: &str) -> bool {
    !source.trim().is_empty() && source.len() <= 100_000 && !source.as_bytes().contains(&0)
}

async fn require_active_terms(state: &AppState, user_id: Uuid) -> Result<(), SubmissionError> {
    let accepted: bool = sqlx::query_scalar(
        "SELECT terms_accepted_at IS NOT NULL FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    if !accepted {
        return Err(SubmissionError::TermsRequired);
    }
    Ok(())
}

async fn submission_view(
    state: &AppState,
    user_id: Uuid,
    submission_id: Uuid,
) -> Result<SubmissionView, SubmissionError> {
    sqlx::query_as::<_, SubmissionView>(
        r#"
        SELECT submission.id, problem.slug AS problem_slug, problem.title_ko AS problem_title,
               submission.language, submission.source, submission.status, submission.score,
               submission.compile_output, submission.run_kind, submission.run_output,
               submission.created_at, submission.judged_at
        FROM submissions submission
        JOIN problems problem ON problem.id = submission.problem_id
        WHERE submission.id = $1
          AND (
              submission.user_id = $2
              OR EXISTS (SELECT 1 FROM user_roles WHERE user_id = $2 AND role = 'ADMIN')
          )
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(SubmissionError::NotFound)
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSubmissionRequest>,
) -> Result<Response, SubmissionError> {
    enqueue(&state, &headers, request, "formal", None).await
}

pub async fn create_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateRunRequest>,
) -> Result<Response, SubmissionError> {
    if !matches!(request.mode.as_str(), "sample" | "custom")
        || (request.mode == "sample" && request.custom_input.is_some())
        || (request.mode == "custom" && request.custom_input.is_none())
        || request
            .custom_input
            .as_ref()
            .is_some_and(|input| input.len() > 65_536)
    {
        return Err(SubmissionError::InvalidInput(
            "샘플 실행 또는 64KB 이하의 사용자 입력을 선택해 주세요",
        ));
    }
    let run_kind = request.mode.clone();
    let submission = CreateSubmissionRequest {
        problem_slug: request.problem_slug,
        language: request.language,
        source: request.source,
        idempotency_key: request.idempotency_key,
        contest_slug: None,
    };
    enqueue(
        &state,
        &headers,
        submission,
        &run_kind,
        request.custom_input,
    )
    .await
}

async fn enqueue(
    state: &AppState,
    headers: &HeaderMap,
    request: CreateSubmissionRequest,
    run_kind: &str,
    custom_input: Option<String>,
) -> Result<Response, SubmissionError> {
    if !valid_language(&request.language) {
        return Err(SubmissionError::InvalidInput(
            "지원 언어는 C++20, Python 3, Java 21입니다",
        ));
    }
    if !valid_source(&request.source) {
        return Err(SubmissionError::InvalidInput(
            "소스 코드는 1바이트 이상 100KB 이하여야 합니다",
        ));
    }
    let user_id = crate::auth::authenticated_user_id_with_csrf(state, headers).await?;
    require_active_terms(state, user_id).await?;
    if let Some(existing_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM submissions WHERE user_id = $1 AND idempotency_key = $2",
    )
    .bind(user_id)
    .bind(request.idempotency_key)
    .fetch_optional(state.pool())
    .await?
    {
        return Ok((
            StatusCode::OK,
            Json(submission_view(state, user_id, existing_id).await?),
        )
            .into_response());
    }

    let mut transaction = state.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(user_id.to_string())
        .execute(&mut *transaction)
        .await?;
    if let Some(existing_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM submissions WHERE user_id = $1 AND idempotency_key = $2",
    )
    .bind(user_id)
    .bind(request.idempotency_key)
    .fetch_optional(&mut *transaction)
    .await?
    {
        transaction.commit().await?;
        return Ok((
            StatusCode::OK,
            Json(submission_view(state, user_id, existing_id).await?),
        )
            .into_response());
    }
    let recent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM submissions WHERE user_id = $1 AND created_at > now() - interval '1 minute'",
    )
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    if recent_count >= 20 {
        return Err(SubmissionError::RateLimited);
    }
    let problem: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, current_revision_id FROM problems WHERE slug = $1 AND status = 'published'",
    )
    .bind(&request.problem_slug)
    .fetch_optional(&mut *transaction)
    .await?;
    let (problem_id, problem_revision_id) = problem.ok_or(SubmissionError::NotFound)?;
    let contest_id = if let Some(contest_slug) = &request.contest_slug {
        if run_kind != "formal" {
            return Err(SubmissionError::ContestForbidden);
        }
        Some(
            sqlx::query_scalar::<_, Uuid>(
                r#"
            SELECT contest.id
            FROM contests contest
            JOIN contest_registrations registration
              ON registration.contest_id = contest.id AND registration.user_id = $1
                 AND registration.status = 'active'
            JOIN contest_problems contest_problem
              ON contest_problem.contest_id = contest.id AND contest_problem.problem_id = $2
            WHERE contest.slug = $3 AND contest.status = 'published'
              AND now() >= contest.starts_at AND now() < contest.ends_at
            "#,
            )
            .bind(user_id)
            .bind(problem_id)
            .bind(contest_slug)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(SubmissionError::ContestForbidden)?,
        )
    } else {
        None
    };
    let submission_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO submissions (
            id, user_id, problem_id, problem_revision_id, language, source,
            idempotency_key, run_kind, custom_input, contest_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(problem_id)
    .bind(problem_revision_id)
    .bind(&request.language)
    .bind(&request.source)
    .bind(request.idempotency_key)
    .bind(run_kind)
    .bind(custom_input)
    .bind(contest_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO judge_jobs (submission_id) VALUES ($1)")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO submission_events (submission_id, status) VALUES ($1, 'QUEUED')")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(submission_view(state, user_id, submission_id).await?),
    )
        .into_response())
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(submission_id): Path<Uuid>,
) -> Result<Json<SubmissionView>, SubmissionError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    Ok(Json(submission_view(&state, user_id, submission_id).await?))
}

pub async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(submission_id): Path<Uuid>,
) -> Result<Json<SubmissionView>, SubmissionError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let mut transaction = state.pool().begin().await?;
    let owned_and_active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM submissions WHERE id=$1 AND user_id=$2 AND judged_at IS NULL)",
    )
    .bind(submission_id)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !owned_and_active {
        return Err(SubmissionError::NotFound);
    }
    sqlx::query(
        r#"
        UPDATE judge_jobs
        SET status='done', lease_owner=NULL, lease_token=NULL, lease_expires_at=NULL,
            updated_at=now(), last_error='사용자 취소'
        WHERE submission_id=$1 AND status IN ('ready', 'leased')
        "#,
    )
    .bind(submission_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE submissions SET status='CANCELLED', score=0, judged_at=now() WHERE id=$1")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO submission_events (submission_id,status,safe_message) VALUES ($1,'CANCELLED','사용자가 판정을 취소했습니다')",
    )
    .bind(submission_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(submission_view(&state, user_id, submission_id).await?))
}

pub async fn rejudge_problem(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(problem_slug): Path<String>,
    Json(request): Json<RejudgeRequest>,
) -> Result<Response, SubmissionError> {
    if !(10..=500).contains(&request.reason.chars().count()) {
        return Err(SubmissionError::InvalidInput(
            "재채점 사유는 10~500자로 입력해 주세요",
        ));
    }
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let authorized: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id=$1 AND role IN ('PROBLEM_SETTER','ADMIN'))",
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !authorized {
        return Err(SubmissionError::RoleForbidden);
    }
    let mut transaction = state.pool().begin().await?;
    let problem: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT id, current_revision_id FROM problems
        WHERE slug = $1 AND status = 'published'
          AND (
              created_by = $2 OR EXISTS (
                  SELECT 1 FROM user_roles
                  WHERE user_id = $2 AND role = 'ADMIN'
              )
          )
        FOR UPDATE
        "#,
    )
    .bind(&problem_slug)
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (problem_id, revision_id) = problem.ok_or(SubmissionError::NotFound)?;
    let request_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO rejudge_requests (id,problem_id,requested_by,reason) VALUES ($1,$2,$3,$4)",
    )
    .bind(request_id)
    .bind(problem_id)
    .bind(user_id)
    .bind(&request.reason)
    .execute(&mut *transaction)
    .await?;
    let submission_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM submissions WHERE problem_id=$1 AND run_kind='formal' AND judged_at IS NOT NULL FOR UPDATE",
    )
    .bind(problem_id)
    .fetch_all(&mut *transaction)
    .await?;
    for submission_id in &submission_ids {
        sqlx::query(
            r#"
            UPDATE submissions SET problem_revision_id=$2, status='QUEUED', score=NULL,
                compile_output=NULL, run_output=NULL, judged_at=NULL
            WHERE id=$1
            "#,
        )
        .bind(submission_id)
        .bind(revision_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE judge_jobs SET status='ready', attempt_count=0, available_at=now(),
                lease_owner=NULL, lease_token=NULL, lease_expires_at=NULL,
                last_error=NULL, updated_at=now()
            WHERE submission_id=$1
            "#,
        )
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO submission_events (submission_id,status,safe_message) VALUES ($1,'QUEUED','관리자 재채점 요청')",
        )
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_events (actor_user_id,action,target_type,target_id,metadata)
        VALUES ($1,'judge.rejudge.requested','problem',$2,$3)
        "#,
    )
    .bind(user_id)
    .bind(problem_id.to_string())
    .bind(serde_json::json!({"request_id": request_id, "submission_count": submission_ids.len()}))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "request_id": request_id,
            "problem_slug": problem_slug,
            "submission_count": submission_ids.len()
        })),
    )
        .into_response())
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(submission_id): Path<Uuid>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, SubmissionError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let can_read: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM submissions submission
            WHERE submission.id = $1
              AND (
                  submission.user_id = $2
                  OR EXISTS (SELECT 1 FROM user_roles WHERE user_id = $2 AND role = 'ADMIN')
              )
        )
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !can_read {
        return Err(SubmissionError::NotFound);
    }

    let pool = state.pool().clone();
    let stream = async_stream::stream! {
        let mut last_event_id = 0_i64;
        for _ in 0..120 {
            let events: Result<Vec<(i64, String, Option<String>)>, sqlx::Error> = sqlx::query_as(
                r#"
                SELECT id, status, safe_message
                FROM submission_events
                WHERE submission_id = $1 AND id > $2
                ORDER BY id
                "#,
            )
            .bind(submission_id)
            .bind(last_event_id)
            .fetch_all(&pool)
            .await;
            let Ok(events) = events else { break };
            let mut terminal = false;
            for (event_id, status, message) in events {
                last_event_id = event_id;
                terminal = matches!(
                    status.as_str(),
                    "ACCEPTED" | "WRONG_ANSWER" | "PARTIAL_ACCEPTED"
                        | "TIME_LIMIT_EXCEEDED" | "MEMORY_LIMIT_EXCEEDED"
                        | "OUTPUT_LIMIT_EXCEEDED" | "RUNTIME_ERROR" | "COMPILE_ERROR"
                        | "SYSTEM_ERROR" | "CANCELLED"
                );
                let data = serde_json::json!({"status": status, "message": message}).to_string();
                yield Ok(Event::default().id(event_id.to_string()).event("status").data(data));
            }
            if terminal { break; }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SubmissionListQuery>,
) -> Result<Json<Vec<SubmissionListItem>>, SubmissionError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let limit = i64::from(query.limit.unwrap_or(30).clamp(1, 100));
    let items = sqlx::query_as::<_, SubmissionListItem>(
        r#"
        SELECT submission.id, problem.slug AS problem_slug, problem.title_ko AS problem_title,
               submission.language, submission.status, submission.score, submission.run_kind,
               submission.created_at, submission.judged_at
        FROM submissions submission
        JOIN problems problem ON problem.id = submission.problem_id
        WHERE submission.user_id = $1 AND ($2::text IS NULL OR problem.slug = $2)
        ORDER BY submission.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(user_id)
    .bind(query.problem_slug)
    .bind(limit)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(items))
}

pub async fn save_draft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(problem_slug): Path<String>,
    Json(request): Json<SaveDraftRequest>,
) -> Result<Json<DraftView>, SubmissionError> {
    if !valid_language(&request.language) || request.source.len() > 100_000 {
        return Err(SubmissionError::InvalidInput(
            "임시 저장 언어 또는 소스 크기를 확인해 주세요",
        ));
    }
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let problem_id: Uuid =
        sqlx::query_scalar("SELECT id FROM problems WHERE slug = $1 AND status = 'published'")
            .bind(&problem_slug)
            .fetch_optional(state.pool())
            .await?
            .ok_or(SubmissionError::NotFound)?;
    sqlx::query(
        r#"
        INSERT INTO source_drafts (user_id, problem_id, language, source)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (user_id, problem_id) DO UPDATE
        SET language = EXCLUDED.language, source = EXCLUDED.source,
            revision_count = source_drafts.revision_count + 1, updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(problem_id)
    .bind(&request.language)
    .bind(&request.source)
    .execute(state.pool())
    .await?;
    get_draft_inner(&state, user_id, &problem_slug)
        .await
        .map(Json)
}

async fn get_draft_inner(
    state: &AppState,
    user_id: Uuid,
    problem_slug: &str,
) -> Result<DraftView, SubmissionError> {
    sqlx::query_as::<_, DraftView>(
        r#"
        SELECT problem.slug AS problem_slug, draft.language, draft.source,
               draft.revision_count, draft.updated_at
        FROM source_drafts draft
        JOIN problems problem ON problem.id = draft.problem_id
        WHERE draft.user_id = $1 AND problem.slug = $2
        "#,
    )
    .bind(user_id)
    .bind(problem_slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(SubmissionError::NotFound)
}

pub async fn get_draft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(problem_slug): Path<String>,
) -> Result<Json<DraftView>, SubmissionError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    get_draft_inner(&state, user_id, &problem_slug)
        .await
        .map(Json)
}
