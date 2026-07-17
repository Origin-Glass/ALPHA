use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ProblemDetail {
    pub id: Uuid,
    pub slug: String,
    #[serde(rename = "title")]
    pub title_ko: String,
    #[serde(rename = "statement")]
    pub statement_ko: String,
    pub difficulty: i16,
    pub learning_axis: String,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub time_limit_ms: i32,
    pub memory_limit_mb: i32,
    pub samples: Vec<SampleCase>,
    pub tags: Vec<ProblemTag>,
}

#[derive(Debug, FromRow)]
struct ProblemRow {
    id: Uuid,
    slug: String,
    title_ko: String,
    statement_ko: String,
    difficulty: i16,
    learning_axis: String,
    source_kind: String,
    source_url: Option<String>,
    time_limit_ms: i32,
    memory_limit_mb: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SampleCase {
    pub ordinal: i32,
    pub input: String,
    pub expected_output: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ProblemTag {
    pub tag: String,
    #[serde(rename = "label")]
    pub label_ko: String,
}

#[derive(Debug, Serialize)]
pub struct ProblemResponse {
    problem: ProblemDetail,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    limit: Option<u16>,
    cursor: Option<Uuid>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ProblemListItem {
    pub id: Uuid,
    pub slug: String,
    #[serde(rename = "title")]
    pub title_ko: String,
    pub difficulty: i16,
    pub learning_axis: String,
    pub source_kind: String,
    pub published_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ProblemListResponse {
    items: Vec<ProblemListItem>,
    next_cursor: Option<Uuid>,
}

#[derive(Debug)]
pub enum ProblemError {
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for ProblemError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"code": "problem_not_found", "message": "문제를 찾을 수 없습니다"}})),
            )
                .into_response(),
            Self::Database(error) => {
                tracing::error!(%error, "문제 조회 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"code": "internal_error", "message": "요청을 처리하지 못했습니다"}})),
                )
                    .into_response()
            }
        }
    }
}

impl From<sqlx::Error> for ProblemError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub async fn list(
    State(pool): State<PgPool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ProblemListResponse>, ProblemError> {
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 100));
    let mut items = sqlx::query_as::<_, ProblemListItem>(
        r#"
        SELECT id, slug, title_ko, difficulty, learning_axis, source_kind, published_at
        FROM problems
        WHERE status = 'published' AND ($1::uuid IS NULL OR id < $1)
        ORDER BY id DESC
        LIMIT $2
        "#,
    )
    .bind(query.cursor)
    .bind(limit + 1)
    .fetch_all(&pool)
    .await?;

    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next_cursor = has_more.then(|| items.last().expect("limit은 1 이상이다").id);

    Ok(Json(ProblemListResponse { items, next_cursor }))
}

pub async fn detail(
    State(pool): State<PgPool>,
    Path(slug): Path<String>,
) -> Result<Json<ProblemResponse>, ProblemError> {
    let row = sqlx::query_as::<_, ProblemRow>(
        r#"
        SELECT id, slug, title_ko, statement_ko, difficulty, learning_axis,
               source_kind, source_url, time_limit_ms, memory_limit_mb
        FROM problems
        WHERE slug = $1 AND status = 'published'
        "#,
    )
    .bind(slug)
    .fetch_optional(&pool)
    .await?
    .ok_or(ProblemError::NotFound)?;

    let samples = sqlx::query_as::<_, SampleCase>(
        r#"
        SELECT ordinal, input, expected_output
        FROM problem_test_cases
        WHERE problem_id = $1 AND visibility = 'sample'
        ORDER BY ordinal
        "#,
    )
    .bind(row.id)
    .fetch_all(&pool)
    .await?;

    let tags = sqlx::query_as::<_, ProblemTag>(
        "SELECT tag, label_ko FROM problem_tags WHERE problem_id = $1 ORDER BY tag",
    )
    .bind(row.id)
    .fetch_all(&pool)
    .await?;

    let problem = ProblemDetail {
        id: row.id,
        slug: row.slug,
        title_ko: row.title_ko,
        statement_ko: row.statement_ko,
        difficulty: row.difficulty,
        learning_axis: row.learning_axis,
        source_kind: row.source_kind,
        source_url: row.source_url,
        time_limit_ms: row.time_limit_ms,
        memory_limit_mb: row.memory_limit_mb,
        samples,
        tags,
    };

    Ok(Json(ProblemResponse { problem }))
}
