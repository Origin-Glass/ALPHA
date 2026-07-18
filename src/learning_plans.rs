use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, types::Json as SqlJson};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

const RULE_VERSION: &str = "project-learning-v1";

#[derive(Debug)]
pub enum LearningPlanError {
    Auth(AuthError),
    Invalid(&'static str),
    NotFound,
    Database(sqlx::Error),
}
impl From<AuthError> for LearningPlanError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}
impl From<sqlx::Error> for LearningPlanError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl IntoResponse for LearningPlanError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(e) => return e.into_response(),
            Self::Invalid(m) => (StatusCode::BAD_REQUEST, "invalid_input", m),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "recommendation_not_found",
                "추천을 찾을 수 없습니다",
            ),
            Self::Database(e) => {
                tracing::error!(%e, "학습 계획 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
        };
        (
            status,
            Json(json!({"error":{"code":code,"message":message}})),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    rule_version: String,
    idempotency_key: Uuid,
    target_outcome: String,
    weekly_minutes: i32,
    preferred_language: String,
    path_mode: String,
    interests: Vec<String>,
    goals: Vec<String>,
    diagnostic_scores: BTreeMap<String, i32>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct PlanResponse {
    id: Uuid,
    revision: i32,
    rule_version: String,
    target_outcome: String,
    weekly_minutes: i32,
    preferred_language: String,
    path_mode: String,
    recommendation_key: String,
    reason_codes: SqlJson<Value>,
    items: SqlJson<Value>,
    plan_hash: String,
    provider_used: bool,
}

#[derive(FromRow)]
struct PlanRow {
    id: Uuid,
    revision: i32,
    rule_version: String,
    target_outcome: String,
    weekly_minutes: i32,
    preferred_language: String,
    path_mode: String,
    recommendation_key: String,
    reason_codes: SqlJson<Value>,
    items: SqlJson<Value>,
    plan_hash: Vec<u8>,
    provider_used: bool,
}
impl From<PlanRow> for PlanResponse {
    fn from(r: PlanRow) -> Self {
        Self {
            id: r.id,
            revision: r.revision,
            rule_version: r.rule_version,
            target_outcome: r.target_outcome,
            weekly_minutes: r.weekly_minutes,
            preferred_language: r.preferred_language,
            path_mode: r.path_mode,
            recommendation_key: r.recommendation_key,
            reason_codes: r.reason_codes,
            items: r.items,
            plan_hash: hex(&r.plan_hash),
            provider_used: r.provider_used,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn bounded(values: &[String], max: usize, each: usize) -> bool {
    !values.is_empty()
        && values.len() <= max
        && values
            .iter()
            .all(|v| !v.trim().is_empty() && v.chars().count() <= each)
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PlanRequest>,
) -> Result<(StatusCode, Json<PlanResponse>), LearningPlanError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if request.rule_version != RULE_VERSION
        || request.target_outcome.trim().is_empty()
        || request.target_outcome.chars().count() > 300
        || !(30..=2400).contains(&request.weekly_minutes)
        || !matches!(request.path_mode.as_str(), "structured" | "exploratory")
        || request.preferred_language.is_empty()
        || request.preferred_language.len() > 32
        || !request
            .preferred_language
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+#.-".contains(c))
        || !bounded(&request.interests, 10, 80)
        || !bounded(&request.goals, 10, 120)
        || request.diagnostic_scores.len() != 4
        || request
            .diagnostic_scores
            .values()
            .any(|v| !(0..=100).contains(v))
    {
        return Err(LearningPlanError::Invalid(
            "학습 계획 입력 범위와 형식을 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(row) = fetch_by_key(&mut tx, user_id, request.idempotency_key).await? {
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(row.into())));
    }
    let rejected: Vec<String> = sqlx::query_scalar(
        "SELECT recommendation_key FROM learning_recommendation_rejections WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    let weakest = request
        .diagnostic_scores
        .iter()
        .min_by_key(|(_, score)| **score)
        .map(|(axis, _)| axis.as_str())
        .unwrap_or("independent_coding");
    let candidates = match weakest {
        "code_literacy" => [
            "code-reading-debugging",
            "independent-project",
            "docs-implementation",
        ],
        "docs_learning" => [
            "docs-implementation",
            "independent-project",
            "code-reading-debugging",
        ],
        _ => [
            "independent-project",
            "code-reading-debugging",
            "docs-implementation",
        ],
    };
    let recommendation = candidates
        .into_iter()
        .find(|c| !rejected.iter().any(|r| r == c))
        .unwrap_or("learner-selected-route");
    let reason_codes = json!([
        format!("mastery_gap:{weakest}"),
        format!("time_budget:{}", request.weekly_minutes),
        format!("path_mode:{}", request.path_mode)
    ]);
    let items = json!([
        {"kind":"project_milestone","title":"작동하는 첫 결과 만들기","estimated_minutes":60,"reason_code":"first_visible_result"},
        {"kind":"code_reading","title":"비슷한 구현 읽고 경계 찾기","estimated_minutes":30,"reason_code":format!("mastery_gap:{weakest}")},
        {"kind":"reflection","title":"선택과 검증 근거 기록","estimated_minutes":30,"reason_code":"auditable_learning"}
    ]);
    let canonical = json!({"rule_version":request.rule_version,"target_outcome":request.target_outcome,"weekly_minutes":request.weekly_minutes,
        "preferred_language":request.preferred_language,"path_mode":request.path_mode,"interests":request.interests,"goals":request.goals,"diagnostic_scores":request.diagnostic_scores});
    let input_bytes = serde_json::to_vec(&canonical)
        .map_err(|_| LearningPlanError::Invalid("학습 계획 입력을 처리할 수 없습니다"))?;
    let input_hash = Sha256::digest(&input_bytes).to_vec();
    let plan_hash = Sha256::digest(serde_json::to_vec(&json!({"input":canonical,"recommendation":recommendation,"reasons":reason_codes,"items":items})).unwrap()).to_vec();
    let previous: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM learning_plan_revisions WHERE user_id=$1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let revision: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision),0)+1 FROM learning_plan_revisions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO learning_plan_revisions (id,user_id,revision,rule_version,idempotency_key,input,input_hash,plan_hash,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,supersedes_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)")
        .bind(id).bind(user_id).bind(revision).bind(RULE_VERSION).bind(request.idempotency_key).bind(canonical).bind(input_hash).bind(&plan_hash)
        .bind(request.target_outcome.trim()).bind(request.weekly_minutes).bind(&request.preferred_language).bind(&request.path_mode).bind(recommendation).bind(reason_codes).bind(items).bind(previous).execute(&mut *tx).await?;
    let row = fetch_by_key(&mut tx, user_id, request.idempotency_key)
        .await?
        .ok_or(LearningPlanError::NotFound)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn fetch_by_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: Uuid,
    key: Uuid,
) -> Result<Option<PlanRow>, sqlx::Error> {
    sqlx::query_as("SELECT id,revision,rule_version,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,plan_hash,provider_used FROM learning_plan_revisions WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(key).fetch_optional(&mut **tx).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRequest {
    reason: String,
    idempotency_key: Uuid,
}
pub async fn reject(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(request): Json<RejectRequest>,
) -> Result<Json<Value>, LearningPlanError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if key.len() > 64
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || request.reason.trim().is_empty()
        || request.reason.chars().count() > 500
    {
        return Err(LearningPlanError::Invalid("추천 거부 사유를 확인해 주세요"));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM learning_plan_revisions WHERE user_id=$1 AND recommendation_key=$2)").bind(user).bind(&key).fetch_one(state.pool()).await?;
    if !exists {
        return Err(LearningPlanError::NotFound);
    }
    sqlx::query("INSERT INTO learning_recommendation_rejections (id,user_id,recommendation_key,reason,idempotency_key) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (user_id,recommendation_key) DO NOTHING")
        .bind(Uuid::now_v7()).bind(user).bind(&key).bind(request.reason.trim()).bind(request.idempotency_key).execute(state.pool()).await?;
    Ok(Json(json!({"recommendation_key":key,"rejected":true})))
}
