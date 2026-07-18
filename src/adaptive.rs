use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{http::AppState, projects::ProjectError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequest {
    milestone_id: Uuid,
    skill: String,
    kind: String,
    successful: bool,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelpRequest {
    milestone_id: Uuid,
    skill: String,
}

fn skill_ok(skill: &str) -> bool {
    !skill.is_empty()
        && skill.len() <= 32
        && skill
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+#.-".contains(c))
}
fn initial_level(policy: &str) -> i16 {
    match policy {
        "guided_ai" => 5,
        "socratic_ai" => 4,
        "documentation_navigator" => 3,
        "curated_documentation" => 2,
        _ => 1,
    }
}

pub async fn evidence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Json(request): Json<EvidenceRequest>,
) -> Result<Json<Value>, ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !skill_ok(&request.skill) || !matches!(request.kind.as_str(), "mastery" | "attempt") {
        return Err(ProjectError::Invalid("도움 근거 형식을 확인해 주세요"));
    }
    let (policy, _) =
        crate::projects::owned_project(state.pool(), user, project, request.milestone_id).await?;
    let mut tx = state.pool().begin().await?;
    if let Some((previous,resulting))=sqlx::query_as::<_,(Option<i16>,Option<i16>)>("SELECT previous_level,resulting_level FROM project_learning_events WHERE user_id=$1 AND idempotency_key=$2")
        .bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await? { tx.commit().await?; return Ok(Json(json!({"previous_level":previous,"level":resulting,"rule_version":"project-learning-v1"}))); }
    sqlx::query("INSERT INTO project_assistance_states (project_id,milestone_id,skill,level) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(project).bind(request.milestone_id).bind(&request.skill).bind(initial_level(&policy)).execute(&mut *tx).await?;
    let (previous,failures):(i16,i16)=sqlx::query_as("SELECT level,consecutive_failures FROM project_assistance_states WHERE project_id=$1 AND milestone_id=$2 AND skill=$3 FOR UPDATE")
        .bind(project).bind(request.milestone_id).bind(&request.skill).fetch_one(&mut *tx).await?;
    let (level, next_failures) = if request.kind == "mastery" && request.successful {
        (previous.saturating_sub(1).max(1), 0)
    } else if !request.successful && failures >= 1 {
        ((previous + 1).min(7), 0)
    } else {
        (previous, failures + 1)
    };
    sqlx::query("UPDATE project_assistance_states SET level=$4,consecutive_failures=$5,updated_at=now() WHERE project_id=$1 AND milestone_id=$2 AND skill=$3")
        .bind(project).bind(request.milestone_id).bind(&request.skill).bind(level).bind(next_failures).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,milestone_id,event_kind,skill,successful,previous_level,resulting_level,idempotency_key,metadata) VALUES ($1,$2,$3,$4,'assistance_evidence',$5,$6,$7,$8,$9,$10)")
        .bind(Uuid::now_v7()).bind(user).bind(project).bind(request.milestone_id).bind(&request.skill).bind(request.successful).bind(previous).bind(level).bind(request.idempotency_key).bind(json!({"kind":request.kind})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"previous_level":previous,"level":level,"rule_version":"project-learning-v1"}),
    ))
}

pub async fn help(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Json(request): Json<HelpRequest>,
) -> Result<Json<Value>, ProjectError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !skill_ok(&request.skill) {
        return Err(ProjectError::Invalid("기술 이름을 확인해 주세요"));
    }
    let (policy, _) =
        crate::projects::owned_project(state.pool(), user, project, request.milestone_id).await?;
    let level:Option<i16>=sqlx::query_scalar("SELECT level FROM project_assistance_states WHERE project_id=$1 AND milestone_id=$2 AND skill=$3")
        .bind(project).bind(request.milestone_id).bind(&request.skill).fetch_optional(state.pool()).await?;
    let level = level.unwrap_or(initial_level(&policy));
    let content = match level {
        1 => "요구사항과 테스트를 확인하고 독립적으로 구현하세요.",
        2 => "승인된 공식 문서와 간단한 치트 시트를 확인하세요.",
        3 => "공식 문서에서 API 이름, 버전, 오류 계약을 먼저 찾으세요.",
        4 => "어떤 입력과 실패 조건부터 검증하면 좋을까요?",
        _ => "목표를 작은 단계로 나누고 다음 한 단계만 구현하세요.",
    };
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,milestone_id,event_kind,skill,resulting_level,metadata) VALUES ($1,$2,$3,$4,'assistance_requested',$5,$6,$7)")
        .bind(Uuid::now_v7()).bind(user).bind(project).bind(request.milestone_id).bind(&request.skill).bind(level).bind(json!({"provider_used":false})).execute(state.pool()).await?;
    Ok(Json(
        json!({"level":level,"content":content,"provider_used":false,"rule_version":"project-learning-v1"}),
    ))
}
