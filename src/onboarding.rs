use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::Json as SqlJson};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

const AXES: [&str; 4] = [
    "algorithmic_reasoning",
    "code_literacy",
    "docs_learning",
    "independent_coding",
];

#[derive(Debug)]
pub enum OnboardingError {
    Auth(AuthError),
    InvalidInput(&'static str),
    TermsRequired,
    PathNotFound,
    DataIntegrity,
    Database(sqlx::Error),
}

impl IntoResponse for OnboardingError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::TermsRequired => (
                StatusCode::FORBIDDEN,
                "terms_required",
                "진단을 완료하기 전에 이용약관에 동의해 주세요",
            ),
            Self::PathNotFound => (
                StatusCode::NOT_FOUND,
                "learning_path_not_found",
                "먼저 학습 진단을 완료해 주세요",
            ),
            Self::DataIntegrity => {
                tracing::error!("온보딩 기준 데이터 무결성 오류");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "학습 진단을 처리하지 못했습니다",
                )
            }
            Self::Database(error) => {
                tracing::error!(%error, "온보딩 데이터베이스 처리 실패");
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

impl From<AuthError> for OnboardingError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for OnboardingError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug, FromRow)]
struct QuestionRow {
    question_key: String,
    axis: String,
    prompt_ko: String,
    options_ko: SqlJson<Vec<String>>,
    option_count: i16,
    correct_option: i16,
}

#[derive(Debug, Serialize)]
pub struct QuestionView {
    question_key: String,
    axis: String,
    prompt: String,
    options: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticAnswer {
    question_key: String,
    selected_option: i16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteRequest {
    answers: Vec<DiagnosticAnswer>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrackView {
    slug: String,
    title: String,
    description: String,
}

#[derive(Debug, FromRow)]
struct TrackRow {
    id: Uuid,
    slug: String,
    title: String,
    description: String,
}

#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    scores: BTreeMap<String, i32>,
    recommended_track: TrackView,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UnitView {
    slug: String,
    title: String,
    summary: String,
    activity_kind: String,
    position: i32,
    estimated_minutes: i32,
    available: bool,
    status: Option<String>,
    mastery_score: Option<i16>,
}

#[derive(Debug, Serialize)]
pub struct LearningPathResponse {
    track: TrackView,
    units: Vec<UnitView>,
}

async fn active_questions(state: &AppState) -> Result<Vec<QuestionRow>, OnboardingError> {
    let questions = sqlx::query_as::<_, QuestionRow>(
        r#"
        SELECT question_key, axis, prompt_ko, options_ko, option_count, correct_option
        FROM diagnostic_questions
        WHERE active = true
        ORDER BY position
        "#,
    )
    .fetch_all(state.pool())
    .await?;

    if questions.len() != AXES.len()
        || questions.iter().any(|question| {
            !AXES.contains(&question.axis.as_str())
                || question.options_ko.len() != question.option_count as usize
        })
    {
        return Err(OnboardingError::DataIntegrity);
    }
    Ok(questions)
}

pub async fn questions(
    State(state): State<AppState>,
) -> Result<Json<Vec<QuestionView>>, OnboardingError> {
    let questions = active_questions(&state)
        .await?
        .into_iter()
        .map(|question| QuestionView {
            question_key: question.question_key,
            axis: question.axis,
            prompt: question.prompt_ko,
            options: question.options_ko.0,
        })
        .collect();
    Ok(Json(questions))
}

pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CompleteRequest>,
) -> Result<Json<CompleteResponse>, OnboardingError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let terms_accepted = sqlx::query_scalar::<_, bool>(
        "SELECT terms_accepted_at IS NOT NULL FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    if !terms_accepted {
        return Err(OnboardingError::TermsRequired);
    }

    let questions = active_questions(&state).await?;
    if request.answers.len() != questions.len() {
        return Err(OnboardingError::InvalidInput(
            "모든 진단 문항에 한 번씩 답해 주세요",
        ));
    }
    let answers: HashMap<&str, i16> = request
        .answers
        .iter()
        .map(|answer| (answer.question_key.as_str(), answer.selected_option))
        .collect();
    if answers.len() != questions.len()
        || questions.iter().any(|question| {
            answers
                .get(question.question_key.as_str())
                .is_none_or(|answer| *answer < 0 || *answer >= question.option_count)
        })
    {
        return Err(OnboardingError::InvalidInput(
            "진단 문항과 답변 선택지를 확인해 주세요",
        ));
    }
    let expected_keys: HashSet<&str> = questions
        .iter()
        .map(|question| question.question_key.as_str())
        .collect();
    if answers.keys().any(|key| !expected_keys.contains(key)) {
        return Err(OnboardingError::InvalidInput(
            "알 수 없는 진단 문항이 포함돼 있습니다",
        ));
    }

    let scores: BTreeMap<String, i32> = questions
        .iter()
        .map(|question| {
            let score =
                i32::from(answers[question.question_key.as_str()] == question.correct_option) * 100;
            (question.axis.clone(), score)
        })
        .collect();
    if scores.len() != AXES.len() {
        return Err(OnboardingError::DataIntegrity);
    }
    let weakest_axis = AXES
        .iter()
        .min_by_key(|axis| scores.get(**axis).copied().unwrap_or(i32::MAX))
        .ok_or(OnboardingError::DataIntegrity)?;

    let mut transaction = state.pool().begin().await?;
    let track = sqlx::query_as::<_, TrackRow>(
        r#"
        SELECT id, slug, title_ko AS title, description_ko AS description
        FROM learning_tracks
        WHERE primary_axis = $1 AND status = 'published'
        "#,
    )
    .bind(weakest_axis)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(OnboardingError::DataIntegrity)?;
    let attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO diagnostic_attempts (id, user_id, scores, recommended_track_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(attempt_id)
    .bind(user_id)
    .bind(serde_json::to_value(&scores).map_err(|_| OnboardingError::DataIntegrity)?)
    .bind(track.id)
    .execute(&mut *transaction)
    .await?;
    for question in &questions {
        let selected_option = answers[question.question_key.as_str()];
        sqlx::query(
            "INSERT INTO diagnostic_answers (attempt_id, question_key, selected_option, correct) VALUES ($1, $2, $3, $4)",
        )
        .bind(attempt_id)
        .bind(&question.question_key)
        .bind(selected_option)
        .bind(selected_option == question.correct_option)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO user_learning_paths (user_id, track_id, source_attempt_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id) DO UPDATE
        SET track_id = EXCLUDED.track_id,
            source_attempt_id = EXCLUDED.source_attempt_id,
            selected_at = now()
        "#,
    )
    .bind(user_id)
    .bind(track.id)
    .bind(attempt_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(Json(CompleteResponse {
        scores,
        recommended_track: TrackView {
            slug: track.slug,
            title: track.title,
            description: track.description,
        },
    }))
}

pub async fn learning_path(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LearningPathResponse>, OnboardingError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let track = sqlx::query_as::<_, TrackView>(
        r#"
        SELECT t.slug, t.title_ko AS title, t.description_ko AS description
        FROM user_learning_paths path
        JOIN learning_tracks t ON t.id = path.track_id
        WHERE path.user_id = $1 AND t.status = 'published'
        "#,
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(OnboardingError::PathNotFound)?;
    let units = sqlx::query_as::<_, UnitView>(
        r#"
        SELECT u.slug, u.title_ko AS title, u.summary_ko AS summary, u.activity_kind,
               u.position, u.estimated_minutes,
               NOT EXISTS (
                   SELECT 1
                   FROM curriculum_prerequisites prerequisite
                   WHERE prerequisite.unit_id = u.id
                     AND NOT EXISTS (
                         SELECT 1
                         FROM user_unit_progress progress
                         WHERE progress.user_id = $1
                           AND progress.unit_id = prerequisite.prerequisite_unit_id
                           AND progress.status = 'completed'
                     )
               ) AS available,
               progress.status,
               progress.mastery_score
        FROM user_learning_paths path
        JOIN curriculum_units u ON u.track_id = path.track_id
        LEFT JOIN user_unit_progress progress ON progress.user_id = $1 AND progress.unit_id = u.id
        WHERE path.user_id = $1 AND u.status = 'published'
        ORDER BY u.position
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;

    Ok(Json(LearningPathResponse { track, units }))
}
