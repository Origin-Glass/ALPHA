use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, types::Json as SqlJson};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

pub trait AiAssistanceProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn enabled(&self) -> bool;
    fn generate(&self, _level: i16) -> Result<String, AssistanceProviderError>;
}

#[derive(Debug)]
pub struct AssistanceProviderError;

pub struct DisabledAiProvider;

impl AiAssistanceProvider for DisabledAiProvider {
    fn name(&self) -> &'static str {
        "disabled"
    }

    fn enabled(&self) -> bool {
        false
    }

    fn generate(&self, _level: i16) -> Result<String, AssistanceProviderError> {
        Err(AssistanceProviderError)
    }
}

pub fn default_ai_provider() -> Arc<dyn AiAssistanceProvider> {
    Arc::new(DisabledAiProvider)
}

#[derive(Debug)]
pub enum ActivityError {
    Auth(AuthError),
    InvalidInput(&'static str),
    NotFound,
    AssistanceLocked,
    AssistancePolicy,
    AiDisabled,
    DataIntegrity,
    Database(sqlx::Error),
}

impl IntoResponse for ActivityError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "activity_not_found",
                "학습 활동을 찾을 수 없습니다",
            ),
            Self::AssistanceLocked => (
                StatusCode::CONFLICT,
                "assistance_level_locked",
                "도움은 현재 단계의 다음 단계까지 차례로 열 수 있습니다",
            ),
            Self::AssistancePolicy => (
                StatusCode::FORBIDDEN,
                "full_explanation_not_allowed",
                "이 활동의 학습 정책은 전체 설명을 허용하지 않습니다",
            ),
            Self::AiDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ai_provider_disabled",
                "AI 도움 제공자가 비활성화되어 있습니다",
            ),
            Self::DataIntegrity => {
                tracing::error!("학습 활동 평가 데이터 무결성 오류");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "학습 활동을 평가하지 못했습니다",
                )
            }
            Self::Database(error) => {
                tracing::error!(%error, "학습 활동 데이터베이스 처리 실패");
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

impl From<AuthError> for ActivityError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for ActivityError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityListQuery {
    kind: Option<String>,
}

#[derive(Debug, FromRow)]
struct ActivityListRow {
    slug: String,
    kind: String,
    title: String,
    instructions: String,
    estimated_minutes: i32,
    status: Option<String>,
    best_score: Option<i16>,
    mastery_class: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActivityListItem {
    slug: String,
    kind: String,
    title: String,
    instructions: String,
    estimated_minutes: i32,
    progress: Option<ActivityProgressView>,
}

#[derive(Debug, Serialize)]
pub struct ActivityProgressView {
    status: String,
    best_score: i16,
    mastery_class: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActivityListResponse {
    items: Vec<ActivityListItem>,
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ActivityListQuery>,
) -> Result<Json<ActivityListResponse>, ActivityError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    if query.kind.as_ref().is_some_and(|kind| {
        !matches!(
            kind.as_str(),
            "predict_output"
                | "trace_state"
                | "explain_behavior"
                | "identify_invariant"
                | "locate_bug"
                | "compare_implementations"
                | "estimate_complexity"
                | "reconstruct_code"
                | "assess_tests"
                | "code_review"
                | "docs_checkpoint"
        )
    }) {
        return Err(ActivityError::InvalidInput("지원하지 않는 활동 종류입니다"));
    }

    let rows = sqlx::query_as::<_, ActivityListRow>(
        r#"
        SELECT activity.slug, activity.kind, activity.title_ko AS title,
               activity.instructions_ko AS instructions, activity.estimated_minutes,
               progress.status, progress.best_score, progress.mastery_class
        FROM learning_activities activity
        LEFT JOIN activity_progress progress
          ON progress.activity_id = activity.id AND progress.user_id = $1
        WHERE activity.status = 'published'
          AND ($2::text IS NULL OR activity.kind = $2)
        ORDER BY activity.created_at, activity.slug
        "#,
    )
    .bind(user_id)
    .bind(query.kind)
    .fetch_all(state.pool())
    .await?;

    Ok(Json(ActivityListResponse {
        items: rows
            .into_iter()
            .map(|row| ActivityListItem {
                slug: row.slug,
                kind: row.kind,
                title: row.title,
                instructions: row.instructions,
                estimated_minutes: row.estimated_minutes,
                progress: row.status.map(|status| ActivityProgressView {
                    status,
                    best_score: row.best_score.unwrap_or_default(),
                    mastery_class: row.mastery_class,
                }),
            })
            .collect(),
    }))
}

#[derive(Debug, FromRow)]
struct ActivityDetailRow {
    id: Uuid,
    slug: String,
    kind: String,
    title: String,
    instructions: String,
    starter_code: Option<String>,
    public_response_schema: SqlJson<Value>,
    estimated_minutes: i32,
    full_explanation_allowed: bool,
    lesson_title: Option<String>,
    mastery_criteria: Option<String>,
    remediation: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ResourceView {
    title: String,
    url: String,
    publisher: String,
    resource_kind: String,
    reading_goal: String,
}

#[derive(Debug, Serialize)]
pub struct ActivityDetailResponse {
    slug: String,
    kind: String,
    title: String,
    instructions: String,
    starter_code: Option<String>,
    response_schema: Value,
    estimated_minutes: i32,
    full_explanation_allowed: bool,
    lesson_title: Option<String>,
    mastery_criteria: Option<String>,
    remediation: Option<String>,
    resources: Vec<ResourceView>,
    progress: Option<DetailedProgressView>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedProgressView {
    status: String,
    attempt_count: i32,
    failed_attempt_count: i32,
    revision_count: i32,
    max_assistance_level: i16,
    best_score: i16,
    mastery_class: Option<String>,
}

async fn find_activity(state: &AppState, slug: &str) -> Result<ActivityDetailRow, ActivityError> {
    sqlx::query_as::<_, ActivityDetailRow>(
        r#"
        SELECT activity.id, activity.slug, activity.kind,
               activity.title_ko AS title, activity.instructions_ko AS instructions,
               activity.starter_code, activity.public_response_schema,
               activity.estimated_minutes, activity.full_explanation_allowed,
               lesson.title_ko AS lesson_title,
               lesson.mastery_criteria_ko AS mastery_criteria,
               lesson.remediation_ko AS remediation
        FROM learning_activities activity
        LEFT JOIN lessons lesson ON lesson.id = activity.lesson_id
        WHERE activity.slug = $1 AND activity.status = 'published'
        "#,
    )
    .bind(slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ActivityError::NotFound)
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<ActivityDetailResponse>, ActivityError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let activity = find_activity(&state, &slug).await?;
    let resources = sqlx::query_as::<_, ResourceView>(
        r#"
        SELECT resource.title_ko AS title, resource.url, resource.publisher,
               resource.resource_kind, link.reading_goal_ko AS reading_goal
        FROM learning_activities activity
        JOIN lesson_resources link ON link.lesson_id = activity.lesson_id
        JOIN documentation_resources resource ON resource.id = link.resource_id
        WHERE activity.id = $1
        ORDER BY link.position
        "#,
    )
    .bind(activity.id)
    .fetch_all(state.pool())
    .await?;
    let progress = sqlx::query_as::<_, DetailedProgressView>(
        r#"
        SELECT status, attempt_count, failed_attempt_count, revision_count,
               max_assistance_level, best_score, mastery_class
        FROM activity_progress WHERE user_id = $1 AND activity_id = $2
        "#,
    )
    .bind(user_id)
    .bind(activity.id)
    .fetch_optional(state.pool())
    .await?;

    Ok(Json(ActivityDetailResponse {
        slug: activity.slug,
        kind: activity.kind,
        title: activity.title,
        instructions: activity.instructions,
        starter_code: activity.starter_code,
        response_schema: activity.public_response_schema.0,
        estimated_minutes: activity.estimated_minutes,
        full_explanation_allowed: activity.full_explanation_allowed,
        lesson_title: activity.lesson_title,
        mastery_criteria: activity.mastery_criteria,
        remediation: activity.remediation,
        resources,
        progress,
    }))
}

#[derive(Debug, Serialize)]
pub struct StartResponse {
    started_at: OffsetDateTime,
}

pub async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<StartResponse>, ActivityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let activity = find_activity(&state, &slug).await?;
    let started_at: OffsetDateTime = sqlx::query_scalar(
        r#"
        INSERT INTO activity_progress (user_id, activity_id)
        VALUES ($1, $2)
        ON CONFLICT (user_id, activity_id) DO UPDATE
        SET last_active_at = now()
        RETURNING started_at
        "#,
    )
    .bind(user_id)
    .bind(activity.id)
    .fetch_one(state.pool())
    .await?;
    Ok(Json(StartResponse { started_at }))
}

#[derive(Debug, FromRow)]
struct EvaluatorRow {
    id: Uuid,
    unit_id: Option<Uuid>,
    evaluator_kind: String,
    evaluator_config: SqlJson<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRequest {
    response: Value,
    reflection: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttemptResponse {
    attempt_id: Uuid,
    score: i16,
    passed: bool,
    mastery_class: Option<String>,
    max_assistance_level: i16,
    elapsed_seconds: i32,
}

fn normalized_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn evaluate(kind: &str, config: &Value, response: &Value) -> Result<i16, ActivityError> {
    match kind {
        "exact_text" => {
            let expected = config.get("expected").and_then(Value::as_str);
            let actual = response.get("answer").and_then(Value::as_str);
            match (expected, actual) {
                (Some(expected), Some(actual)) => {
                    Ok(i16::from(normalized_text(expected) == normalized_text(actual)) * 100)
                }
                (Some(_), None) => Err(ActivityError::InvalidInput("답변 형식을 확인해 주세요")),
                _ => Err(ActivityError::DataIntegrity),
            }
        }
        "single_choice" => {
            let expected = config.get("expected").and_then(Value::as_str);
            let actual = response.get("choice").and_then(Value::as_str);
            match (expected, actual) {
                (Some(expected), Some(actual)) => Ok(i16::from(expected == actual) * 100),
                (Some(_), None) => Err(ActivityError::InvalidInput("선택지를 골라 주세요")),
                _ => Err(ActivityError::DataIntegrity),
            }
        }
        "structured_fields" => {
            let expected = config.get("expected").and_then(Value::as_object);
            let actual = response.get("fields").and_then(Value::as_object);
            match (expected, actual) {
                (Some(expected), Some(actual)) if !expected.is_empty() => {
                    let correct = expected
                        .iter()
                        .filter(|(key, value)| {
                            let expected = value.as_str().map(normalized_text);
                            let actual = actual
                                .get(*key)
                                .and_then(Value::as_str)
                                .map(normalized_text);
                            expected.is_some() && expected == actual
                        })
                        .count();
                    Ok(((correct * 100) / expected.len()) as i16)
                }
                (Some(_), None) => Err(ActivityError::InvalidInput("구조화 답변을 채워 주세요")),
                _ => Err(ActivityError::DataIntegrity),
            }
        }
        "ordered_tokens" => {
            let expected = config.get("expected").and_then(Value::as_array);
            let actual = response.get("tokens").and_then(Value::as_array);
            match (expected, actual) {
                (Some(expected), Some(actual)) => Ok(i16::from(expected == actual) * 100),
                (Some(_), None) => Err(ActivityError::InvalidInput("토큰 답변을 채워 주세요")),
                _ => Err(ActivityError::DataIntegrity),
            }
        }
        _ => Err(ActivityError::DataIntegrity),
    }
}

fn mastery_class(level: i16) -> &'static str {
    match level {
        0 => "independent",
        1..=6 => "assisted",
        7..=8 => "reviewed",
        _ => "explained",
    }
}

pub async fn attempt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<AttemptRequest>,
) -> Result<Json<AttemptResponse>, ActivityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if serde_json::to_vec(&request.response)
        .map_err(|_| ActivityError::InvalidInput("답변 형식을 확인해 주세요"))?
        .len()
        > 16_384
    {
        return Err(ActivityError::InvalidInput("답변은 16KB 이하여야 합니다"));
    }
    if request
        .reflection
        .as_ref()
        .is_some_and(|value| value.chars().count() > 2000)
    {
        return Err(ActivityError::InvalidInput(
            "회고는 2,000자 이하여야 합니다",
        ));
    }

    let evaluator = sqlx::query_as::<_, EvaluatorRow>(
        r#"
        SELECT id, unit_id, evaluator_kind, evaluator_config
        FROM learning_activities
        WHERE slug = $1 AND status = 'published'
        "#,
    )
    .bind(&slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ActivityError::NotFound)?;
    let score = evaluate(
        &evaluator.evaluator_kind,
        &evaluator.evaluator_config.0,
        &request.response,
    )?;
    let passed = score >= 80;

    let mut transaction = state.pool().begin().await?;
    sqlx::query(
        "INSERT INTO activity_progress (user_id, activity_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(evaluator.id)
    .execute(&mut *transaction)
    .await?;
    let (started_at, max_level): (OffsetDateTime, i16) = sqlx::query_as(
        "SELECT started_at, max_assistance_level FROM activity_progress WHERE user_id = $1 AND activity_id = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(evaluator.id)
    .fetch_one(&mut *transaction)
    .await?;
    let elapsed_seconds = (OffsetDateTime::now_utc() - started_at)
        .whole_seconds()
        .clamp(0, 86_400) as i32;
    let classification = passed.then(|| mastery_class(max_level).to_owned());
    let attempt_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO activity_attempts (
            id, user_id, activity_id, response, score, passed, max_assistance_level,
            mastery_class, elapsed_seconds, reflection
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(attempt_id)
    .bind(user_id)
    .bind(evaluator.id)
    .bind(&request.response)
    .bind(score)
    .bind(passed)
    .bind(max_level)
    .bind(&classification)
    .bind(elapsed_seconds)
    .bind(&request.reflection)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE activity_progress
        SET attempt_count = attempt_count + 1,
            failed_attempt_count = failed_attempt_count + CASE WHEN $3 THEN 0 ELSE 1 END,
            revision_count = revision_count + 1,
            best_score = GREATEST(best_score, $2),
            status = CASE WHEN GREATEST(best_score, $2) >= 80 THEN 'completed' ELSE 'started' END,
            mastery_class = CASE
                WHEN GREATEST(best_score, $2) >= 80 THEN $4
                ELSE NULL
            END,
            explicit_reflection = COALESCE($5, explicit_reflection),
            completed_at = CASE
                WHEN GREATEST(best_score, $2) >= 80 THEN COALESCE(completed_at, now())
                ELSE NULL
            END,
            last_active_at = now()
        WHERE user_id = $1 AND activity_id = $6
        "#,
    )
    .bind(user_id)
    .bind(score)
    .bind(passed)
    .bind(mastery_class(max_level))
    .bind(&request.reflection)
    .bind(evaluator.id)
    .execute(&mut *transaction)
    .await?;

    if let Some(unit_id) = evaluator.unit_id {
        let all_completed: bool = sqlx::query_scalar(
            r#"
            SELECT NOT EXISTS (
                SELECT 1 FROM learning_activities activity
                WHERE activity.unit_id = $2 AND activity.status = 'published'
                  AND NOT EXISTS (
                      SELECT 1 FROM activity_progress progress
                      WHERE progress.user_id = $1 AND progress.activity_id = activity.id
                        AND progress.status = 'completed'
                  )
            )
            "#,
        )
        .bind(user_id)
        .bind(unit_id)
        .fetch_one(&mut *transaction)
        .await?;
        if all_completed {
            let unit_score: i16 = sqlx::query_scalar(
                r#"
                SELECT ROUND(AVG(progress.best_score))::smallint
                FROM learning_activities activity
                JOIN activity_progress progress ON progress.activity_id = activity.id
                WHERE activity.unit_id = $2 AND progress.user_id = $1 AND activity.status = 'published'
                "#,
            )
            .bind(user_id)
            .bind(unit_id)
            .fetch_one(&mut *transaction)
            .await?;
            let coarse_assistance = match max_level {
                0 => "none",
                1..=5 => "hint",
                6 => "guided",
                _ => "ai_assisted",
            };
            sqlx::query(
                r#"
                INSERT INTO user_unit_progress (
                    user_id, unit_id, status, mastery_score, assistance_level, completed_at
                ) VALUES ($1, $2, 'completed', $3, $4, now())
                ON CONFLICT (user_id, unit_id) DO UPDATE
                SET status = 'completed', mastery_score = GREATEST(user_unit_progress.mastery_score, EXCLUDED.mastery_score),
                    assistance_level = EXCLUDED.assistance_level,
                    completed_at = COALESCE(user_unit_progress.completed_at, now())
                "#,
            )
            .bind(user_id)
            .bind(unit_id)
            .bind(unit_score)
            .bind(coarse_assistance)
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;

    Ok(Json(AttemptResponse {
        attempt_id,
        score,
        passed,
        mastery_class: classification,
        max_assistance_level: max_level,
        elapsed_seconds,
    }))
}

#[derive(Debug, Serialize)]
pub struct AssistanceResponse {
    level: i16,
    kind: String,
    content: String,
    provider: String,
}

pub async fn assistance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, level)): Path<(String, i16)>,
) -> Result<Json<AssistanceResponse>, ActivityError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !(1..=9).contains(&level) {
        return Err(ActivityError::InvalidInput(
            "도움 단계는 1에서 9 사이여야 합니다",
        ));
    }
    let activity = find_activity(&state, &slug).await?;
    let mut transaction = state.pool().begin().await?;
    sqlx::query(
        "INSERT INTO activity_progress (user_id, activity_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(activity.id)
    .execute(&mut *transaction)
    .await?;
    let (started_at, current_level): (OffsetDateTime, i16) = sqlx::query_as(
        "SELECT started_at, max_assistance_level FROM activity_progress WHERE user_id = $1 AND activity_id = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(activity.id)
    .fetch_one(&mut *transaction)
    .await?;
    if level > current_level + 1 {
        return Err(ActivityError::AssistanceLocked);
    }
    if level == 9 && !activity.full_explanation_allowed {
        return Err(ActivityError::AssistancePolicy);
    }
    let elapsed_seconds = (OffsetDateTime::now_utc() - started_at)
        .whole_seconds()
        .clamp(0, 86_400) as i32;

    if level >= 7 {
        let kind = match level {
            7 => "reasoning_critique",
            8 => "code_review",
            _ => "full_explanation",
        };
        sqlx::query(
            r#"
            INSERT INTO assistance_events (
                user_id, activity_id, level, assistance_kind, provider, elapsed_seconds
            ) VALUES ($1, $2, $3, $4, 'disabled_ai', $5)
            "#,
        )
        .bind(user_id)
        .bind(activity.id)
        .bind(level)
        .bind(kind)
        .bind(elapsed_seconds)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return match state.ai_provider().generate(level) {
            Ok(content) => Ok(Json(AssistanceResponse {
                level,
                kind: kind.to_owned(),
                content,
                provider: state.ai_provider().name().to_owned(),
            })),
            Err(_) => Err(ActivityError::AiDisabled),
        };
    }

    let (kind, content): (String, String) = sqlx::query_as(
        "SELECT kind, content_ko FROM activity_assistance_content WHERE activity_id = $1 AND level = $2",
    )
    .bind(activity.id)
    .bind(level)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ActivityError::DataIntegrity)?;
    sqlx::query(
        r#"
        INSERT INTO assistance_events (
            user_id, activity_id, level, assistance_kind, provider, elapsed_seconds
        ) VALUES ($1, $2, $3, $4, 'deterministic', $5)
        "#,
    )
    .bind(user_id)
    .bind(activity.id)
    .bind(level)
    .bind(&kind)
    .bind(elapsed_seconds)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE activity_progress
        SET max_assistance_level = GREATEST(max_assistance_level, $3), last_active_at = now(),
            mastery_class = CASE
                WHEN best_score >= 80 THEN CASE WHEN GREATEST(max_assistance_level, $3) <= 6 THEN 'assisted' ELSE mastery_class END
                ELSE NULL
            END
        WHERE user_id = $1 AND activity_id = $2
        "#,
    )
    .bind(user_id)
    .bind(activity.id)
    .bind(level)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(Json(AssistanceResponse {
        level,
        kind,
        content,
        provider: "deterministic".to_owned(),
    }))
}

#[derive(Debug, Serialize)]
pub struct AiStatusResponse {
    enabled: bool,
    provider: String,
    available_levels: Vec<i16>,
    ai_levels: Vec<i16>,
}

pub async fn ai_status(State(state): State<AppState>) -> Json<AiStatusResponse> {
    Json(AiStatusResponse {
        enabled: state.ai_provider().enabled(),
        provider: state.ai_provider().name().to_owned(),
        available_levels: (1..=6).collect(),
        ai_levels: vec![7, 8, 9],
    })
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackHeader {
    slug: String,
    title: String,
    description: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackModuleView {
    slug: String,
    title: String,
    goal: String,
    position: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackLessonView {
    module_slug: String,
    slug: String,
    title: String,
    goal: String,
    mastery_criteria: String,
    remediation: String,
    position: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackResourceView {
    lesson_slug: String,
    title: String,
    url: String,
    publisher: String,
    reading_goal: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackActivityView {
    lesson_slug: String,
    slug: String,
    kind: String,
    title: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackCapstoneView {
    lesson_slug: String,
    problem_slug: String,
    title: String,
    exercise_kind: String,
}

#[derive(Debug, Serialize)]
pub struct TrackDetailResponse {
    track: TrackHeader,
    modules: Vec<TrackModuleView>,
    lessons: Vec<TrackLessonView>,
    resources: Vec<TrackResourceView>,
    activities: Vec<TrackActivityView>,
    capstones: Vec<TrackCapstoneView>,
}

pub async fn track(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<TrackDetailResponse>, ActivityError> {
    let track = sqlx::query_as::<_, TrackHeader>(
        "SELECT slug, title_ko AS title, description_ko AS description FROM learning_tracks WHERE slug = $1 AND status = 'published'",
    )
    .bind(&slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ActivityError::NotFound)?;
    let modules = sqlx::query_as::<_, TrackModuleView>(
        r#"
        SELECT module.slug, module.title_ko AS title, module.goal_ko AS goal, module.position
        FROM learning_modules module
        JOIN learning_tracks track ON track.id = module.track_id
        WHERE track.slug = $1 ORDER BY module.position
        "#,
    )
    .bind(&slug)
    .fetch_all(state.pool())
    .await?;
    let lessons = sqlx::query_as::<_, TrackLessonView>(
        r#"
        SELECT module.slug AS module_slug, lesson.slug, lesson.title_ko AS title,
               lesson.goal_ko AS goal, lesson.mastery_criteria_ko AS mastery_criteria,
               lesson.remediation_ko AS remediation, lesson.position
        FROM lessons lesson
        JOIN learning_modules module ON module.id = lesson.module_id
        JOIN learning_tracks track ON track.id = module.track_id
        WHERE track.slug = $1 ORDER BY module.position, lesson.position
        "#,
    )
    .bind(&slug)
    .fetch_all(state.pool())
    .await?;
    let resources = sqlx::query_as::<_, TrackResourceView>(
        r#"
        SELECT lesson.slug AS lesson_slug, resource.title_ko AS title, resource.url,
               resource.publisher, link.reading_goal_ko AS reading_goal
        FROM lesson_resources link
        JOIN lessons lesson ON lesson.id = link.lesson_id
        JOIN learning_modules module ON module.id = lesson.module_id
        JOIN learning_tracks track ON track.id = module.track_id
        JOIN documentation_resources resource ON resource.id = link.resource_id
        WHERE track.slug = $1 ORDER BY module.position, lesson.position, link.position
        "#,
    )
    .bind(&slug)
    .fetch_all(state.pool())
    .await?;
    let activities = sqlx::query_as::<_, TrackActivityView>(
        r#"
        SELECT lesson.slug AS lesson_slug, activity.slug, activity.kind, activity.title_ko AS title
        FROM learning_activities activity
        JOIN lessons lesson ON lesson.id = activity.lesson_id
        JOIN learning_modules module ON module.id = lesson.module_id
        JOIN learning_tracks track ON track.id = module.track_id
        WHERE track.slug = $1 AND activity.status = 'published'
        ORDER BY module.position, lesson.position, activity.created_at
        "#,
    )
    .bind(&slug)
    .fetch_all(state.pool())
    .await?;
    let capstones = sqlx::query_as::<_, TrackCapstoneView>(
        r#"
        SELECT lesson.slug AS lesson_slug, problem.slug AS problem_slug,
               problem.title_ko AS title, exercise.exercise_kind
        FROM lesson_problem_exercises exercise
        JOIN lessons lesson ON lesson.id = exercise.lesson_id
        JOIN learning_modules module ON module.id = lesson.module_id
        JOIN learning_tracks track ON track.id = module.track_id
        JOIN problems problem ON problem.id = exercise.problem_id
        WHERE track.slug = $1 AND problem.status = 'published'
        ORDER BY module.position, lesson.position, exercise.position
        "#,
    )
    .bind(&slug)
    .fetch_all(state.pool())
    .await?;

    Ok(Json(TrackDetailResponse {
        track,
        modules,
        lessons,
        resources,
        activities,
        capstones,
    }))
}
