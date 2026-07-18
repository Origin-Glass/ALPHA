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
    Forbidden,
    Conflict,
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
            Self::Conflict => (
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "같은 멱등키에 다른 요청을 사용할 수 없습니다",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "template_assignment_forbidden",
                "이 학습 계획 템플릿은 현재 사용자에게 배정되지 않았습니다",
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
    deadline: String,
    preferred_framework: String,
    desired_project: String,
    required_curriculum: Vec<String>,
    instructor_constraints: Vec<String>,
    assessment_checkpoints: Vec<String>,
    assistance_policy: String,
    privacy: String,
    origin: String,
    template_id: Option<Uuid>,
    locked_requirements: Vec<String>,
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
    restored_from_id: Option<Uuid>,
    deadline: String,
    preferred_framework: String,
    desired_project: String,
    required_curriculum: SqlJson<Value>,
    instructor_constraints: SqlJson<Value>,
    assessment_checkpoints: SqlJson<Value>,
    assistance_policy: String,
    privacy: String,
    origin: String,
    template_id: Option<Uuid>,
    locked_requirements: SqlJson<Value>,
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
    restored_from_id: Option<Uuid>,
    deadline: String,
    preferred_framework: String,
    desired_project: String,
    required_curriculum: SqlJson<Value>,
    instructor_constraints: SqlJson<Value>,
    assessment_checkpoints: SqlJson<Value>,
    assistance_policy: String,
    privacy: String,
    origin: String,
    template_id: Option<Uuid>,
    locked_requirements: SqlJson<Value>,
}
#[derive(FromRow)]
struct RestoreRow {
    id: Uuid,
    rule_version: String,
    input: Value,
    input_hash: Vec<u8>,
    plan_hash: Vec<u8>,
    target_outcome: String,
    weekly_minutes: i32,
    preferred_language: String,
    path_mode: String,
    recommendation_key: String,
    reason_codes: SqlJson<Value>,
    items: SqlJson<Value>,
    provider_used: bool,
    deadline: String,
    preferred_framework: String,
    desired_project: String,
    required_curriculum: SqlJson<Value>,
    instructor_constraints: SqlJson<Value>,
    assessment_checkpoints: SqlJson<Value>,
    assistance_policy: String,
    privacy: String,
    origin: String,
    template_id: Option<Uuid>,
    locked_requirements: SqlJson<Value>,
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
            restored_from_id: r.restored_from_id,
            deadline: r.deadline,
            preferred_framework: r.preferred_framework,
            desired_project: r.desired_project,
            required_curriculum: r.required_curriculum,
            instructor_constraints: r.instructor_constraints,
            assessment_checkpoints: r.assessment_checkpoints,
            assistance_policy: r.assistance_policy,
            privacy: r.privacy,
            origin: r.origin,
            template_id: r.template_id,
            locked_requirements: r.locked_requirements,
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
fn text_bound(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}
fn valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let parsed = (
        parts.next().and_then(|v| v.parse::<i32>().ok()),
        parts.next().and_then(|v| v.parse::<u8>().ok()),
        parts.next().and_then(|v| v.parse::<u8>().ok()),
    );
    matches!(parsed, (Some(year), Some(month), Some(day))
        if value.len() == 10
            && parts.next().is_none()
            && time::Month::try_from(month).ok().and_then(|month| time::Date::from_calendar_date(year, month, day).ok()).is_some())
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PlanRequest>,
) -> Result<(StatusCode, Json<PlanResponse>), LearningPlanError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    const REQUIRED_AXES: [&str; 4] = [
        "algorithmic_reasoning",
        "code_literacy",
        "docs_learning",
        "independent_coding",
    ];
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
        || REQUIRED_AXES
            .iter()
            .any(|axis| !request.diagnostic_scores.contains_key(*axis))
        || request
            .diagnostic_scores
            .values()
            .any(|v| !(0..=100).contains(v))
        || !valid_date(&request.deadline)
        || !text_bound(&request.preferred_framework, 64)
        || !text_bound(&request.desired_project, 200)
        || !bounded(&request.required_curriculum, 12, 120)
        || request.instructor_constraints.len() > 10
        || request
            .instructor_constraints
            .iter()
            .any(|v| !text_bound(v, 200))
        || !bounded(&request.assessment_checkpoints, 12, 160)
        || !matches!(
            request.assistance_policy.as_str(),
            "guided_ai"
                | "socratic_ai"
                | "documentation_navigator"
                | "curated_documentation"
                | "cheat_sheet_only"
                | "independent"
                | "transfer_challenge"
        )
        || !matches!(
            request.privacy.as_str(),
            "private" | "instructors" | "classroom"
        )
        || !matches!(request.origin.as_str(), "learner" | "template_assignment")
        || request.locked_requirements.len() > 12
        || request
            .locked_requirements
            .iter()
            .any(|v| !text_bound(v, 160))
        || (request.origin == "learner"
            && (request.template_id.is_some() || !request.locked_requirements.is_empty()))
        || (request.origin == "template_assignment"
            && (request.template_id.is_none() || request.locked_requirements.is_empty()))
    {
        return Err(LearningPlanError::Invalid(
            "학습 계획 입력 범위와 형식을 확인해 주세요",
        ));
    }
    let canonical = json!({"rule_version":request.rule_version,"target_outcome":request.target_outcome,"weekly_minutes":request.weekly_minutes,
        "preferred_language":request.preferred_language,"path_mode":request.path_mode,"interests":request.interests,"goals":request.goals,"diagnostic_scores":request.diagnostic_scores,
        "deadline":request.deadline,"preferred_framework":request.preferred_framework,"desired_project":request.desired_project,"required_curriculum":request.required_curriculum,
        "instructor_constraints":request.instructor_constraints,"assessment_checkpoints":request.assessment_checkpoints,"assistance_policy":request.assistance_policy,"privacy":request.privacy,
        "origin":request.origin,"template_id":request.template_id,"locked_requirements":request.locked_requirements});
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(template_id) = request.template_id {
        let assigned: Option<SqlJson<Value>> = sqlx::query_scalar(
            "SELECT assignment.locked_requirements FROM learning_plan_assignments assignment JOIN learning_plan_templates template ON template.id=assignment.template_id WHERE assignment.user_id=$1 AND assignment.template_id=$2 AND assignment.revoked_at IS NULL AND template.active=true",
        )
        .bind(user_id)
        .bind(template_id)
        .fetch_optional(&mut *tx)
        .await?;
        let assigned = assigned.ok_or(LearningPlanError::Forbidden)?;
        if assigned.0 != json!(request.locked_requirements) {
            return Err(LearningPlanError::Invalid(
                "배정된 템플릿의 잠긴 요구사항과 정확히 일치해야 합니다",
            ));
        }
    }
    if let Some(existing) = sqlx::query_scalar::<_, SqlJson<Value>>(
        "SELECT input FROM learning_plan_revisions WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(user_id)
    .bind(request.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if existing.0 != canonical {
            return Err(LearningPlanError::Conflict);
        }
        let row = fetch_by_key(&mut tx, user_id, request.idempotency_key)
            .await?
            .ok_or(LearningPlanError::NotFound)?;
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
    sqlx::query("INSERT INTO learning_plan_revisions (id,user_id,revision,rule_version,idempotency_key,input,input_hash,plan_hash,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,supersedes_id,deadline,preferred_framework,desired_project,required_curriculum,instructor_constraints,assessment_checkpoints,assistance_policy,privacy,origin,template_id,locked_requirements) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17::date,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27)")
        .bind(id).bind(user_id).bind(revision).bind(RULE_VERSION).bind(request.idempotency_key).bind(canonical).bind(input_hash).bind(&plan_hash)
        .bind(request.target_outcome.trim()).bind(request.weekly_minutes).bind(&request.preferred_language).bind(&request.path_mode).bind(recommendation).bind(reason_codes).bind(items).bind(previous)
        .bind(&request.deadline).bind(&request.preferred_framework).bind(&request.desired_project).bind(SqlJson(json!(request.required_curriculum))).bind(SqlJson(json!(request.instructor_constraints))).bind(SqlJson(json!(request.assessment_checkpoints))).bind(&request.assistance_policy).bind(&request.privacy).bind(&request.origin).bind(request.template_id).bind(SqlJson(json!(request.locked_requirements))).execute(&mut *tx).await?;
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
    sqlx::query_as("SELECT id,revision,rule_version,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,plan_hash,provider_used,restored_from_id,deadline::text AS deadline,preferred_framework,desired_project,required_curriculum,instructor_constraints,assessment_checkpoints,assistance_policy,privacy,origin,template_id,locked_requirements FROM learning_plan_revisions WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(key).fetch_optional(&mut **tx).await
}

pub async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, LearningPlanError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let rows:Vec<PlanRow>=sqlx::query_as("SELECT id,revision,rule_version,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,plan_hash,provider_used,restored_from_id,deadline::text AS deadline,preferred_framework,desired_project,required_curriculum,instructor_constraints,assessment_checkpoints,assistance_policy,privacy,origin,template_id,locked_requirements FROM learning_plan_revisions WHERE user_id=$1 ORDER BY revision DESC")
        .bind(user).fetch_all(state.pool()).await?;
    Ok(Json(
        json!({"revisions":rows.into_iter().map(PlanResponse::from).collect::<Vec<_>>()}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    idempotency_key: Uuid,
}
pub async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(revision): Path<i32>,
    Json(request): Json<RestoreRequest>,
) -> Result<(StatusCode, Json<PlanResponse>), LearningPlanError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if revision < 1 {
        return Err(LearningPlanError::Invalid(
            "복원할 계획 리비전을 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(row) = fetch_by_key(&mut tx, user, request.idempotency_key).await? {
        let source: Option<i32> = sqlx::query_scalar(
            "SELECT revision FROM learning_plan_revisions WHERE id=$1 AND user_id=$2",
        )
        .bind(row.restored_from_id)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if source != Some(revision) {
            return Err(LearningPlanError::Conflict);
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(row.into())));
    }
    let source:RestoreRow=sqlx::query_as("SELECT id,rule_version,input,input_hash,plan_hash,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,provider_used,deadline::text AS deadline,preferred_framework,desired_project,required_curriculum,instructor_constraints,assessment_checkpoints,assistance_policy,privacy,origin,template_id,locked_requirements FROM learning_plan_revisions WHERE user_id=$1 AND revision=$2")
        .bind(user).bind(revision).fetch_optional(&mut *tx).await?.ok_or(LearningPlanError::NotFound)?;
    let latest: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM learning_plan_revisions WHERE user_id=$1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    let next: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision),0)+1 FROM learning_plan_revisions WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO learning_plan_revisions (id,user_id,revision,rule_version,idempotency_key,input,input_hash,plan_hash,target_outcome,weekly_minutes,preferred_language,path_mode,recommendation_key,reason_codes,items,provider_used,supersedes_id,restored_from_id,deadline,preferred_framework,desired_project,required_curriculum,instructor_constraints,assessment_checkpoints,assistance_policy,privacy,origin,template_id,locked_requirements) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19::date,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29)")
        .bind(id).bind(user).bind(next).bind(source.rule_version).bind(request.idempotency_key).bind(source.input).bind(source.input_hash).bind(source.plan_hash).bind(source.target_outcome).bind(source.weekly_minutes).bind(source.preferred_language).bind(source.path_mode).bind(source.recommendation_key).bind(source.reason_codes).bind(source.items).bind(source.provider_used).bind(latest).bind(source.id)
        .bind(source.deadline).bind(source.preferred_framework).bind(source.desired_project).bind(source.required_curriculum).bind(source.instructor_constraints).bind(source.assessment_checkpoints).bind(source.assistance_policy).bind(source.privacy).bind(source.origin).bind(source.template_id).bind(source.locked_requirements).execute(&mut *tx).await?;
    let row = fetch_by_key(&mut tx, user, request.idempotency_key)
        .await?
        .ok_or(LearningPlanError::NotFound)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
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
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((saved_key,saved_reason))=sqlx::query_as::<_,(String,String)>("SELECT recommendation_key,reason FROM learning_recommendation_rejections WHERE user_id=$1 AND idempotency_key=$2").bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await?{
        if saved_key!=key||saved_reason!=request.reason.trim(){return Err(LearningPlanError::Conflict);}
        tx.commit().await?;return Ok(Json(json!({"recommendation_key":key,"rejected":true,"already_rejected":true})));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM learning_plan_revisions WHERE user_id=$1 AND recommendation_key=$2)").bind(user).bind(&key).fetch_one(&mut *tx).await?;
    if !exists {
        return Err(LearningPlanError::NotFound);
    }
    let prior:Option<String>=sqlx::query_scalar("SELECT reason FROM learning_recommendation_rejections WHERE user_id=$1 AND recommendation_key=$2").bind(user).bind(&key).fetch_optional(&mut *tx).await?;
    if prior.is_some() {
        return Err(LearningPlanError::Conflict);
    }
    sqlx::query("INSERT INTO learning_recommendation_rejections (id,user_id,recommendation_key,reason,idempotency_key) VALUES ($1,$2,$3,$4,$5)")
        .bind(Uuid::now_v7()).bind(user).bind(&key).bind(request.reason.trim()).bind(request.idempotency_key).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"recommendation_key":key,"rejected":true,"already_rejected":false}),
    ))
}
