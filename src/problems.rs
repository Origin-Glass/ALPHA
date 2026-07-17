use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::Json as SqlJson};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::http::AppState;

#[derive(Debug, Serialize)]
pub struct ProblemDetail {
    pub id: Uuid,
    pub slug: String,
    #[serde(rename = "title")]
    pub title_ko: String,
    #[serde(rename = "statement")]
    pub statement_ko: String,
    pub difficulty: Option<i16>,
    pub difficulty_source: String,
    pub normalized_tier: Option<String>,
    pub learning_axis: String,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub time_limit_ms: i32,
    pub memory_limit_mb: i32,
    pub samples: Vec<SampleCase>,
    pub tags: Vec<ProblemTag>,
    pub external_metadata: Option<ExternalMetadataView>,
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

#[derive(Debug, Clone, Deserialize, Serialize, FromRow)]
pub struct ProblemTag {
    pub tag: String,
    #[serde(rename = "label")]
    pub label_ko: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ExternalMetadataView {
    provider: String,
    external_problem_id: String,
    raw_level: Option<i16>,
    normalized_tier: Option<String>,
    tags: SqlJson<Vec<ProblemTag>>,
    fetched_at: OffsetDateTime,
    state: String,
    source_url: String,
    attribution: String,
}

#[derive(Debug, Serialize)]
pub struct ProblemResponse {
    problem: ProblemDetail,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    limit: Option<u16>,
    cursor: Option<Uuid>,
    q: Option<String>,
    tag: Option<String>,
    min_level: Option<i16>,
    max_level: Option<i16>,
    axis: Option<String>,
    source: Option<String>,
}

#[derive(Debug, FromRow)]
struct ProblemListRow {
    id: Uuid,
    slug: String,
    title_ko: String,
    difficulty: Option<i16>,
    difficulty_source: String,
    normalized_tier: Option<String>,
    learning_axis: String,
    source_kind: String,
    published_at: OffsetDateTime,
    metadata_state: Option<String>,
    attribution: Option<String>,
    tags: SqlJson<Vec<ProblemTag>>,
}

#[derive(Debug, Serialize)]
pub struct ProblemListItem {
    pub id: Uuid,
    pub slug: String,
    #[serde(rename = "title")]
    pub title_ko: String,
    pub difficulty: Option<i16>,
    pub difficulty_source: String,
    pub normalized_tier: Option<String>,
    pub learning_axis: String,
    pub source_kind: String,
    pub published_at: OffsetDateTime,
    pub metadata_state: Option<String>,
    pub attribution: Option<String>,
    pub tags: Vec<ProblemTag>,
}

impl From<ProblemListRow> for ProblemListItem {
    fn from(row: ProblemListRow) -> Self {
        Self {
            id: row.id,
            slug: row.slug,
            title_ko: row.title_ko,
            difficulty: row.difficulty,
            difficulty_source: row.difficulty_source,
            normalized_tier: row.normalized_tier,
            learning_axis: row.learning_axis,
            source_kind: row.source_kind,
            published_at: row.published_at,
            metadata_state: row.metadata_state,
            attribution: row.attribution,
            tags: row.tags.0,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProblemListResponse {
    items: Vec<ProblemListItem>,
    next_cursor: Option<Uuid>,
}

#[derive(Debug)]
pub enum ProblemError {
    InvalidInput(&'static str),
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for ProblemError {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidInput(message) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"code": "invalid_input", "message": message}})),
            )
                .into_response(),
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

fn valid_filter_token(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ProblemListResponse>, ProblemError> {
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 100));
    let search = query
        .q
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 100)
    {
        return Err(ProblemError::InvalidInput("검색어는 100자 이하여야 합니다"));
    }
    if query
        .tag
        .as_ref()
        .is_some_and(|tag| !valid_filter_token(tag))
    {
        return Err(ProblemError::InvalidInput("태그 형식이 올바르지 않습니다"));
    }
    if query.axis.as_ref().is_some_and(|axis| {
        !matches!(
            axis.as_str(),
            "algorithmic_reasoning" | "code_literacy" | "docs_learning" | "independent_coding"
        )
    }) {
        return Err(ProblemError::InvalidInput("학습 축이 올바르지 않습니다"));
    }
    if query.source.as_ref().is_some_and(|source| {
        !matches!(
            source.as_str(),
            "original" | "licensed" | "external_metadata"
        )
    }) {
        return Err(ProblemError::InvalidInput("문제 출처가 올바르지 않습니다"));
    }
    if query
        .min_level
        .is_some_and(|level| !(0..=30).contains(&level))
        || query
            .max_level
            .is_some_and(|level| !(0..=30).contains(&level))
        || matches!((query.min_level, query.max_level), (Some(minimum), Some(maximum)) if minimum > maximum)
    {
        return Err(ProblemError::InvalidInput(
            "난이도 범위는 0~30 안에서 순서대로 입력해 주세요",
        ));
    }

    let mut rows = sqlx::query_as::<_, ProblemListRow>(
        r#"
        SELECT p.id, p.slug, p.title_ko,
               CASE WHEN external.problem_id IS NULL THEN p.difficulty ELSE metadata.raw_level END AS difficulty,
               CASE WHEN external.problem_id IS NULL THEN 'alpha' ELSE external.provider END AS difficulty_source,
               CASE WHEN external.problem_id IS NULL THEN NULL ELSE metadata.normalized_tier END AS normalized_tier,
               p.learning_axis, p.source_kind, p.published_at,
               metadata.state AS metadata_state,
               external.attribution_ko AS attribution,
               COALESCE(
                   CASE WHEN external.problem_id IS NOT NULL THEN metadata.normalized_tags END,
                   (
                       SELECT jsonb_agg(jsonb_build_object('tag', tag.tag, 'label', tag.label_ko) ORDER BY tag.tag)
                       FROM problem_tags tag WHERE tag.problem_id = p.id
                   ),
                   '[]'::jsonb
               ) AS tags
        FROM problems p
        LEFT JOIN problem_external_links external ON external.problem_id = p.id
        LEFT JOIN external_problem_metadata_cache metadata
               ON metadata.provider = external.provider
              AND metadata.external_problem_id = external.external_problem_id
        WHERE p.status = 'published'
          AND ($1::uuid IS NULL OR p.id < $1)
          AND ($2::text IS NULL OR p.title_ko ILIKE '%' || $2 || '%' OR p.slug ILIKE '%' || $2 || '%')
          AND (
              $3::text IS NULL
              OR EXISTS (SELECT 1 FROM problem_tags tag WHERE tag.problem_id = p.id AND tag.tag = $3)
              OR EXISTS (
                  SELECT 1 FROM jsonb_array_elements(COALESCE(metadata.normalized_tags, '[]'::jsonb)) item
                  WHERE item->>'tag' = $3
              )
          )
          AND ($4::smallint IS NULL OR (CASE WHEN external.problem_id IS NULL THEN p.difficulty ELSE metadata.raw_level END) >= $4)
          AND ($5::smallint IS NULL OR (CASE WHEN external.problem_id IS NULL THEN p.difficulty ELSE metadata.raw_level END) <= $5)
          AND ($6::text IS NULL OR p.learning_axis = $6)
          AND ($7::text IS NULL OR p.source_kind = $7)
        ORDER BY p.id DESC
        LIMIT $8
        "#,
    )
    .bind(query.cursor)
    .bind(search)
    .bind(query.tag)
    .bind(query.min_level)
    .bind(query.max_level)
    .bind(query.axis)
    .bind(query.source)
    .bind(limit + 1)
    .fetch_all(state.pool())
    .await?;

    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| rows.last().expect("limit은 1 이상이다").id);
    let items = rows.into_iter().map(ProblemListItem::from).collect();

    Ok(Json(ProblemListResponse { items, next_cursor }))
}

pub async fn detail(
    State(state): State<AppState>,
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
    .fetch_optional(state.pool())
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
    .fetch_all(state.pool())
    .await?;

    let external_metadata = sqlx::query_as::<_, ExternalMetadataView>(
        r#"
        SELECT external.provider, external.external_problem_id, metadata.raw_level,
               metadata.normalized_tier, COALESCE(metadata.normalized_tags, '[]'::jsonb) AS tags,
               metadata.fetched_at, metadata.state, metadata.source_url,
               external.attribution_ko AS attribution
        FROM problem_external_links external
        JOIN external_problem_metadata_cache metadata
          ON metadata.provider = external.provider
         AND metadata.external_problem_id = external.external_problem_id
        WHERE external.problem_id = $1
        "#,
    )
    .bind(row.id)
    .fetch_optional(state.pool())
    .await?;
    let local_tags = sqlx::query_as::<_, ProblemTag>(
        "SELECT tag, label_ko FROM problem_tags WHERE problem_id = $1 ORDER BY tag",
    )
    .bind(row.id)
    .fetch_all(state.pool())
    .await?;
    let (difficulty, difficulty_source, normalized_tier, tags) = external_metadata
        .as_ref()
        .map(|metadata| {
            (
                metadata.raw_level,
                metadata.provider.clone(),
                metadata.normalized_tier.clone(),
                metadata.tags.0.clone(),
            )
        })
        .unwrap_or((Some(row.difficulty), "alpha".to_owned(), None, local_tags));

    let problem = ProblemDetail {
        id: row.id,
        slug: row.slug,
        title_ko: row.title_ko,
        statement_ko: row.statement_ko,
        difficulty,
        difficulty_source,
        normalized_tier,
        learning_axis: row.learning_axis,
        source_kind: row.source_kind,
        source_url: row.source_url,
        time_limit_ms: row.time_limit_ms,
        memory_limit_mb: row.memory_limit_mb,
        samples,
        tags,
        external_metadata,
    };

    Ok(Json(ProblemResponse { problem }))
}
