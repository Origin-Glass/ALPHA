use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, types::Json as SqlJson};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum ProjectError {
    Auth(AuthError),
    Invalid(&'static str),
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}
impl From<AuthError> for ProjectError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}
impl From<sqlx::Error> for ProjectError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl IntoResponse for ProjectError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(e) => return e.into_response(),
            Self::Invalid(m) => (StatusCode::BAD_REQUEST, "invalid_input", m),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "project_not_found",
                "프로젝트 또는 아이디어를 찾을 수 없습니다",
            ),
            Self::Conflict(m) => (StatusCode::CONFLICT, "project_conflict", m),
            Self::Database(e) => {
                tracing::error!(%e, "프로젝트 처리 실패");
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdeaRequest {
    title: String,
    motivation: String,
    target_user: String,
    intended_outcome: String,
    core_feature: String,
    technology: String,
    weekly_minutes: i32,
    requested_features: Vec<String>,
    assistance_policy: String,
    idempotency_key: Uuid,
}

#[derive(Debug, Serialize, FromRow)]
pub struct IdeaResponse {
    id: Uuid,
    revision: i32,
    title: String,
    motivation: String,
    target_user: String,
    intended_outcome: String,
    core_feature: String,
    technology: String,
    weekly_minutes: i32,
    assistance_policy: String,
    #[sqlx(rename = "scoped_features")]
    features: SqlJson<Value>,
    milestones: SqlJson<Value>,
    scope_reduced: bool,
    provider_used: bool,
    rule_version: String,
}

fn text_ok(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}
fn technology_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+#.-".contains(c))
}
fn policy_ok(value: &str) -> bool {
    matches!(
        value,
        "guided_ai"
            | "socratic_ai"
            | "documentation_navigator"
            | "curated_documentation"
            | "cheat_sheet_only"
            | "independent"
            | "transfer_challenge"
    )
}

pub async fn create_idea(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<IdeaRequest>,
) -> Result<(StatusCode, Json<IdeaResponse>), ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !text_ok(&request.title, 120)
        || !text_ok(&request.motivation, 500)
        || !text_ok(&request.target_user, 200)
        || !text_ok(&request.intended_outcome, 300)
        || !text_ok(&request.core_feature, 120)
        || !technology_ok(&request.technology)
        || !(30..=2400).contains(&request.weekly_minutes)
        || !policy_ok(&request.assistance_policy)
        || request.requested_features.is_empty()
        || request.requested_features.len() > 20
        || request.requested_features.iter().any(|f| !text_ok(f, 120))
    {
        return Err(ProjectError::Invalid(
            "프로젝트 아이디어 입력 범위와 형식을 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(row) = idea_by_key(&mut tx, user, request.idempotency_key).await? {
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(row)));
    }
    let features: Vec<String> = request.requested_features.iter().take(5).cloned().collect();
    let milestone_count = features.len().clamp(1, 3);
    let milestones:Vec<Value>=(0..milestone_count).map(|index| json!({
        "position":index+1,
        "title":if index==0 { format!("{} 작동 결과 확인",request.core_feature) } else { format!("{} 확장 {}",request.core_feature,index+1) },
        "estimated_minutes":if index==0 { 60 } else { 90 },
        "visible_result":true
    })).collect();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO project_ideas (id,user_id,idempotency_key,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,requested_features,scoped_features,milestones,scope_reduced,rule_version) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,'project-learning-v1')")
        .bind(id).bind(user).bind(request.idempotency_key).bind(request.title.trim()).bind(request.motivation.trim()).bind(request.target_user.trim())
        .bind(request.intended_outcome.trim()).bind(request.core_feature.trim()).bind(&request.technology).bind(request.weekly_minutes).bind(&request.assistance_policy)
        .bind(SqlJson(json!(request.requested_features))).bind(SqlJson(json!(features))).bind(SqlJson(json!(milestones))).bind(request.requested_features.len()>features.len()).execute(&mut *tx).await?;
    let row = idea_by_key(&mut tx, user, request.idempotency_key)
        .await?
        .ok_or(ProjectError::NotFound)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn idea_by_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: Uuid,
    key: Uuid,
) -> Result<Option<IdeaResponse>, sqlx::Error> {
    sqlx::query_as("SELECT id,revision,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,scoped_features,milestones,scope_reduced,provider_used,rule_version FROM project_ideas WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(key).fetch_optional(&mut **tx).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    idea_id: Uuid,
    idempotency_key: Uuid,
}
#[derive(Debug, Serialize, FromRow)]
pub struct MilestoneView {
    pub id: Uuid,
    position: i16,
    title: String,
    estimated_minutes: i32,
    visible_result: bool,
    status: String,
}
#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: Uuid,
    idea_id: Uuid,
    status: String,
    title: String,
    technology: String,
    assistance_policy: String,
    pub milestones: Vec<MilestoneView>,
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM learner_projects WHERE user_id=$1 AND (idempotency_key=$2 OR idea_id=$3)",
    )
    .bind(user)
    .bind(request.idempotency_key)
    .bind(request.idea_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        let result = project_detail(&mut tx, user, id).await?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(result)));
    }
    let idea:(String,String,String,SqlJson<Value>)=sqlx::query_as("SELECT title,technology,assistance_policy,milestones FROM project_ideas WHERE id=$1 AND user_id=$2")
        .bind(request.idea_id).bind(user).fetch_optional(&mut *tx).await?.ok_or(ProjectError::NotFound)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO learner_projects (id,user_id,idea_id,idempotency_key) VALUES ($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(user)
    .bind(request.idea_id)
    .bind(request.idempotency_key)
    .execute(&mut *tx)
    .await?;
    let milestones = idea.3.0.as_array().ok_or(ProjectError::Conflict(
        "아이디어 마일스톤이 올바르지 않습니다",
    ))?;
    for milestone in milestones {
        sqlx::query("INSERT INTO project_milestones (id,project_id,position,title,estimated_minutes,visible_result) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(Uuid::now_v7()).bind(id).bind(milestone["position"].as_i64().ok_or(ProjectError::Conflict("마일스톤 순서가 없습니다"))? as i16)
            .bind(milestone["title"].as_str().ok_or(ProjectError::Conflict("마일스톤 제목이 없습니다"))?)
            .bind(milestone["estimated_minutes"].as_i64().ok_or(ProjectError::Conflict("마일스톤 시간이 없습니다"))? as i32)
            .bind(milestone["visible_result"].as_bool().unwrap_or(false)).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,event_kind,idempotency_key,metadata) VALUES ($1,$2,$3,'project_created',$4,$5)")
        .bind(Uuid::now_v7()).bind(user).bind(id).bind(request.idempotency_key).bind(json!({"idea_id":request.idea_id})).execute(&mut *tx).await?;
    let result = project_detail(&mut tx, user, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn project_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: Uuid,
    id: Uuid,
) -> Result<ProjectResponse, ProjectError> {
    let row:(Uuid,String,String,String,String)=sqlx::query_as("SELECT p.idea_id,p.status,i.title,i.technology,i.assistance_policy FROM learner_projects p JOIN project_ideas i ON i.id=p.idea_id WHERE p.id=$1 AND p.user_id=$2")
        .bind(id).bind(user).fetch_optional(&mut **tx).await?.ok_or(ProjectError::NotFound)?;
    let milestones=sqlx::query_as("SELECT id,position,title,estimated_minutes,visible_result,status FROM project_milestones WHERE project_id=$1 ORDER BY position").bind(id).fetch_all(&mut **tx).await?;
    Ok(ProjectResponse {
        id,
        idea_id: row.0,
        status: row.1,
        title: row.2,
        technology: row.3,
        assistance_policy: row.4,
        milestones,
    })
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, ProjectError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let mut tx = state.pool().begin().await?;
    let result = project_detail(&mut tx, user, id).await?;
    tx.commit().await?;
    Ok(Json(result))
}

pub(crate) async fn owned_project(
    pool: &sqlx::PgPool,
    user: Uuid,
    project: Uuid,
    milestone: Uuid,
) -> Result<(String, String), ProjectError> {
    sqlx::query_as("SELECT i.assistance_policy,i.technology FROM learner_projects p JOIN project_ideas i ON i.id=p.idea_id JOIN project_milestones m ON m.project_id=p.id WHERE p.id=$1 AND p.user_id=$2 AND m.id=$3")
        .bind(project).bind(user).bind(milestone).fetch_optional(pool).await?.ok_or(ProjectError::NotFound)
}
