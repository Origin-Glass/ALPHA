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
    skill_level: String,
    runtime: String,
    infrastructure: String,
    source_kind: String,
    repository_url: Option<String>,
    repository_revision: Option<String>,
    ownership_basis: Option<String>,
    license_identifier: Option<String>,
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
    feasibility_reasons: SqlJson<Value>,
    excluded_features: SqlJson<Value>,
    scope_reduced: bool,
    provider_used: bool,
    rule_version: String,
    source_kind: String,
    repository_url: Option<String>,
    repository_revision: Option<String>,
    ownership_basis: Option<String>,
    license_identifier: Option<String>,
    lineage_id: Uuid,
    supersedes_id: Option<Uuid>,
    restored_from_id: Option<Uuid>,
    confirmation_required: bool,
}

fn text_ok(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_repository_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
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
        || !matches!(
            request.skill_level.as_str(),
            "beginner" | "intermediate" | "advanced"
        )
        || !matches!(request.runtime.as_str(), "browser" | "cli" | "server")
        || !matches!(
            request.infrastructure.as_str(),
            "local_only" | "container_available"
        )
        || request.requested_features.is_empty()
        || request.requested_features.len() > 20
        || request.requested_features.iter().any(|f| !text_ok(f, 120))
        || !request.requested_features.contains(&request.core_feature)
    {
        return Err(ProjectError::Invalid(
            "프로젝트 아이디어 입력 범위와 형식을 확인해 주세요",
        ));
    }
    let import_complete = request.source_kind == "imported"
        && request
            .repository_url
            .as_ref()
            .is_some_and(|v| valid_repository_url(v))
        && request.repository_revision.as_ref().is_some_and(|v| {
            matches!(v.len(), 40 | 64)
                && v.chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
        && matches!(
            request.ownership_basis.as_deref(),
            Some("learner_owned" | "authorized_import")
        )
        && request.license_identifier.as_ref().is_some_and(|v| {
            (2..=40).contains(&v.len())
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
        });
    if !matches!(request.source_kind.as_str(), "original" | "imported")
        || (request.source_kind == "imported" && !import_complete)
        || (request.source_kind == "original"
            && [
                request.repository_url.as_ref(),
                request.repository_revision.as_ref(),
                request.ownership_basis.as_ref(),
                request.license_identifier.as_ref(),
            ]
            .iter()
            .any(|v| v.is_some()))
    {
        return Err(ProjectError::Invalid(
            "가져온 프로젝트는 HTTPS 저장소, 정확한 리비전, 소유권과 라이선스 근거가 모두 필요합니다",
        ));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    let canonical = json!({"title":request.title.trim(),"motivation":request.motivation.trim(),"target_user":request.target_user.trim(),
        "intended_outcome":request.intended_outcome.trim(),"core_feature":request.core_feature.trim(),"technology":request.technology,
        "weekly_minutes":request.weekly_minutes,"assistance_policy":request.assistance_policy,"requested_features":request.requested_features,
        "skill_level":request.skill_level,"runtime":request.runtime,"infrastructure":request.infrastructure,"source_kind":request.source_kind,
        "repository_url":request.repository_url,"repository_revision":request.repository_revision,"ownership_basis":request.ownership_basis,"license_identifier":request.license_identifier});
    if let Some(existing) = sqlx::query_scalar::<_, SqlJson<Value>>("SELECT jsonb_build_object('title',title,'motivation',motivation,'target_user',target_user,'intended_outcome',intended_outcome,'core_feature',core_feature,'technology',technology,'weekly_minutes',weekly_minutes,'assistance_policy',assistance_policy,'requested_features',requested_features,'skill_level',skill_level,'runtime',runtime,'infrastructure',infrastructure,'source_kind',source_kind,'repository_url',repository_url,'repository_revision',repository_revision,'ownership_basis',ownership_basis,'license_identifier',license_identifier) FROM project_ideas WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await? {
        if existing.0 != canonical { return Err(ProjectError::Conflict("같은 멱등키에 다른 아이디어를 사용할 수 없습니다")); }
        let row = idea_by_key(&mut tx, user, request.idempotency_key).await?.ok_or(ProjectError::NotFound)?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(row)));
    }
    let mut candidates = request.requested_features.clone();
    candidates.sort();
    candidates.dedup();
    candidates.sort_by_key(|feature| (feature != &request.core_feature, feature.clone()));
    let capacity = if request.weekly_minutes <= 90 || request.skill_level == "beginner" {
        3
    } else {
        5
    };
    let features: Vec<String> = candidates.iter().take(capacity).cloned().collect();
    let excluded_features:Vec<Value>=candidates.iter().skip(capacity).map(|feature|json!({"feature":feature,"reason_code":if request.weekly_minutes<=90{"time_budget"}else{"scope_limit"}})).collect();
    let feasibility_reasons = json!([
        format!("weekly_minutes:{}", request.weekly_minutes),
        format!("skill_level:{}", request.skill_level),
        format!("runtime:{}", request.runtime),
        format!("infrastructure:{}", request.infrastructure)
    ]);
    let milestone_count = features.len().clamp(1, 3);
    let activity_slugs: Vec<String> = sqlx::query_scalar(
        "SELECT slug FROM learning_activities WHERE status='published' ORDER BY slug LIMIT 3",
    )
    .fetch_all(&mut *tx)
    .await?;
    if activity_slugs.is_empty() {
        return Err(ProjectError::Conflict(
            "게시된 학습 활동 요구사항이 없습니다",
        ));
    }
    let milestones:Vec<Value>=(0..milestone_count).map(|index| json!({
        "position":index+1,
        "title":if index==0 { format!("{} 작동 결과 확인",request.core_feature) } else { format!("{} 확장 {}",request.core_feature,index+1) },
        "estimated_minutes":if index==0 { 60 } else { 90 },
        "visible_result":true,"required_activity_slug":activity_slugs[index%activity_slugs.len()]
    })).collect();
    let id = Uuid::now_v7();
    let input_hash = Sha256::digest(
        serde_json::to_vec(&canonical)
            .map_err(|_| ProjectError::Invalid("아이디어 입력을 처리할 수 없습니다"))?,
    )
    .to_vec();
    sqlx::query("INSERT INTO project_ideas (id,user_id,idempotency_key,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,skill_level,runtime,infrastructure,requested_features,scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,rule_version,source_kind,repository_url,repository_revision,ownership_basis,license_identifier,lineage_id,input,input_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,'project-learning-v1',$21,$22,$23,$24,$25,$26,$27,$28)")
        .bind(id).bind(user).bind(request.idempotency_key).bind(request.title.trim()).bind(request.motivation.trim()).bind(request.target_user.trim())
        .bind(request.intended_outcome.trim()).bind(request.core_feature.trim()).bind(&request.technology).bind(request.weekly_minutes).bind(&request.assistance_policy).bind(&request.skill_level).bind(&request.runtime).bind(&request.infrastructure)
        .bind(SqlJson(json!(request.requested_features))).bind(SqlJson(json!(features))).bind(SqlJson(json!(milestones))).bind(SqlJson(feasibility_reasons)).bind(SqlJson(json!(excluded_features))).bind(request.requested_features.len()>features.len())
        .bind(&request.source_kind).bind(&request.repository_url).bind(&request.repository_revision).bind(&request.ownership_basis).bind(&request.license_identifier).bind(id).bind(SqlJson(canonical)).bind(input_hash).execute(&mut *tx).await?;
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
    sqlx::query_as("SELECT id,revision,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,provider_used,rule_version,source_kind,repository_url,repository_revision,ownership_basis,license_identifier,lineage_id,supersedes_id,restored_from_id,true AS confirmation_required FROM project_ideas WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(key).fetch_optional(&mut **tx).await
}

pub async fn idea_history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ProjectError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let rows:Vec<IdeaResponse>=sqlx::query_as("SELECT i.id,i.revision,i.title,i.motivation,i.target_user,i.intended_outcome,i.core_feature,i.technology,i.weekly_minutes,i.assistance_policy,i.scoped_features,i.milestones,i.feasibility_reasons,i.excluded_features,i.scope_reduced,i.provider_used,i.rule_version,i.source_kind,i.repository_url,i.repository_revision,i.ownership_basis,i.license_identifier,i.lineage_id,i.supersedes_id,i.restored_from_id,NOT EXISTS(SELECT 1 FROM project_idea_confirmations c WHERE c.idea_id=i.id AND c.user_id=i.user_id) AS confirmation_required FROM project_ideas i WHERE i.user_id=$1 ORDER BY i.lineage_id,i.revision DESC")
        .bind(user).fetch_all(state.pool()).await?;
    Ok(Json(json!({"ideas":rows})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreIdeaRequest {
    idempotency_key: Uuid,
}

pub async fn restore_idea(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(source_id): Path<Uuid>,
    Json(request): Json<RestoreIdeaRequest>,
) -> Result<(StatusCode, Json<IdeaResponse>), ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(row) = idea_by_key(&mut tx, user, request.idempotency_key).await? {
        if row.restored_from_id != Some(source_id) {
            return Err(ProjectError::Conflict(
                "같은 멱등키에 다른 아이디어 복원을 사용할 수 없습니다",
            ));
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(row)));
    }
    let lineage: Uuid =
        sqlx::query_scalar("SELECT lineage_id FROM project_ideas WHERE id=$1 AND user_id=$2")
            .bind(source_id)
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ProjectError::NotFound)?;
    let (supersedes, revision): (Uuid, i32) = sqlx::query_as(
        "SELECT id,revision+1 FROM project_ideas WHERE lineage_id=$1 AND user_id=$2 ORDER BY revision DESC LIMIT 1",
    )
    .bind(lineage)
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO project_ideas (id,user_id,revision,idempotency_key,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,skill_level,runtime,infrastructure,requested_features,scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,provider_used,rule_version,source_kind,repository_url,repository_revision,ownership_basis,license_identifier,lineage_id,supersedes_id,restored_from_id,input,input_hash) SELECT $1,user_id,$2,$3,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,skill_level,runtime,infrastructure,requested_features,scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,provider_used,rule_version,source_kind,repository_url,repository_revision,ownership_basis,license_identifier,lineage_id,$4,$5,input,input_hash FROM project_ideas WHERE id=$5 AND user_id=$6")
        .bind(id).bind(revision).bind(request.idempotency_key).bind(supersedes).bind(source_id).bind(user).execute(&mut *tx).await?;
    let row = idea_by_key(&mut tx, user, request.idempotency_key)
        .await?
        .ok_or(ProjectError::NotFound)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmMilestone {
    position: i16,
    title: String,
    estimated_minutes: i32,
    visible_result: bool,
    required_activity_slug: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmIdeaRequest {
    scoped_features: Vec<String>,
    milestones: Vec<ConfirmMilestone>,
    idempotency_key: Uuid,
}

pub async fn confirm_idea(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ConfirmIdeaRequest>,
) -> Result<Json<Value>, ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if request.scoped_features.is_empty()
        || request.scoped_features.len() > 5
        || request.scoped_features.iter().any(|v| !text_ok(v, 120))
        || request.milestones.is_empty()
        || request.milestones.len() > 3
        || request.milestones.iter().enumerate().any(|(index, m)| {
            m.position != (index + 1) as i16
                || !text_ok(&m.title, 160)
                || !(30..=120).contains(&m.estimated_minutes)
                || !m.visible_result
                || m.required_activity_slug.len() > 64
                || !m
                    .required_activity_slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    {
        return Err(ProjectError::Invalid(
            "확정 기능과 마일스톤 범위를 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    let (requested,core,server_milestones):(SqlJson<Vec<String>>,String,SqlJson<Value>)=sqlx::query_as("SELECT requested_features,core_feature,milestones FROM project_ideas WHERE id=$1 AND user_id=$2")
        .bind(id).bind(user).fetch_optional(&mut *tx).await?.ok_or(ProjectError::NotFound)?;
    let unique: std::collections::HashSet<&str> =
        request.scoped_features.iter().map(String::as_str).collect();
    let submitted_milestones = serde_json::to_value(&request.milestones)
        .map_err(|_| ProjectError::Invalid("범위 확정을 처리할 수 없습니다"))?;
    if submitted_milestones != server_milestones.0
        || unique.len() != request.scoped_features.len()
        || !unique.contains(core.as_str())
        || request
            .scoped_features
            .iter()
            .any(|v| !requested.0.contains(v))
    {
        return Err(ProjectError::Invalid(
            "확정 기능은 원래 제안의 중복 없는 부분집합이며 핵심 기능과 서버 활동 요구사항을 포함해야 합니다",
        ));
    }
    let confirmation =
        json!({"scoped_features":request.scoped_features,"milestones":request.milestones});
    let confirmation_hash = Sha256::digest(
        serde_json::to_vec(&confirmation)
            .map_err(|_| ProjectError::Invalid("범위 확정을 처리할 수 없습니다"))?,
    )
    .to_vec();
    if let Some((idea,features,milestones))=sqlx::query_as::<_,(Uuid,SqlJson<Value>,SqlJson<Value>)>("SELECT idea_id,scoped_features,milestones FROM project_idea_confirmations WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await?{
        if idea!=id||features.0!=confirmation["scoped_features"]||milestones.0!=confirmation["milestones"]{return Err(ProjectError::Conflict("같은 멱등키에 다른 범위 확정을 사용할 수 없습니다"));}
        tx.commit().await?;return Ok(Json(json!({"idea_id":id,"confirmed":true,"confirmation_required":false,"confirmation_hash":hex(&confirmation_hash)})));
    }
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_idea_confirmations WHERE idea_id=$1 AND user_id=$2)",
    )
    .bind(id)
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    if already {
        return Err(ProjectError::Conflict(
            "이미 확정한 아이디어 범위는 새 리비전으로 다시 생성해 주세요",
        ));
    }
    sqlx::query("INSERT INTO project_idea_confirmations (id,idea_id,user_id,scoped_features,milestones,confirmation_hash,idempotency_key) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(Uuid::now_v7()).bind(id).bind(user).bind(SqlJson(confirmation["scoped_features"].clone())).bind(SqlJson(confirmation["milestones"].clone())).bind(&confirmation_hash).bind(request.idempotency_key).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"idea_id":id,"confirmed":true,"confirmation_required":false,"confirmation_hash":hex(&confirmation_hash)}),
    ))
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
    required_activity_slug: String,
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
    if let Some((id, idea_id, key)) = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        "SELECT id,idea_id,idempotency_key FROM learner_projects WHERE user_id=$1 AND (idempotency_key=$2 OR idea_id=$3)",
    )
    .bind(user)
    .bind(request.idempotency_key)
    .bind(request.idea_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if key == request.idempotency_key && idea_id != request.idea_id {
            return Err(ProjectError::Conflict("같은 멱등키에 다른 아이디어를 사용할 수 없습니다"));
        }
        let result = project_detail(&mut tx, user, id).await?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(result)));
    }
    let idea:(String,String,String,SqlJson<Value>)=sqlx::query_as("SELECT i.title,i.technology,i.assistance_policy,c.milestones FROM project_ideas i JOIN project_idea_confirmations c ON c.idea_id=i.id AND c.user_id=i.user_id WHERE i.id=$1 AND i.user_id=$2")
        .bind(request.idea_id).bind(user).fetch_optional(&mut *tx).await?.ok_or(ProjectError::Conflict("프로젝트를 만들기 전에 축소 범위와 마일스톤을 명시적으로 확정해 주세요"))?;
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
        let required_activity_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM learning_activities WHERE slug=$1 AND status='published'",
        )
        .bind(
            milestone["required_activity_slug"]
                .as_str()
                .ok_or(ProjectError::Conflict("마일스톤 활동 요구사항이 없습니다"))?,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ProjectError::Conflict(
            "마일스톤 활동 요구사항을 찾을 수 없습니다",
        ))?;
        sqlx::query("INSERT INTO project_milestones (id,project_id,user_id,position,title,estimated_minutes,visible_result,required_activity_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(Uuid::now_v7()).bind(id).bind(user).bind(milestone["position"].as_i64().ok_or(ProjectError::Conflict("마일스톤 순서가 없습니다"))? as i16)
            .bind(milestone["title"].as_str().ok_or(ProjectError::Conflict("마일스톤 제목이 없습니다"))?)
            .bind(milestone["estimated_minutes"].as_i64().ok_or(ProjectError::Conflict("마일스톤 시간이 없습니다"))? as i32)
            .bind(milestone["visible_result"].as_bool().unwrap_or(false)).bind(required_activity_id).execute(&mut *tx).await?;
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
    let milestones=sqlx::query_as("SELECT m.id,m.position,m.title,m.estimated_minutes,m.visible_result,m.status,a.slug AS required_activity_slug FROM project_milestones m JOIN learning_activities a ON a.id=m.required_activity_id WHERE m.project_id=$1 AND m.user_id=$2 ORDER BY m.position").bind(id).bind(user).fetch_all(&mut **tx).await?;
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
