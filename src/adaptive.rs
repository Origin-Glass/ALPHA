use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::types::Json as SqlJson;
use uuid::Uuid;

use crate::{http::AppState, projects::ProjectError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequest {
    milestone_id: Uuid,
    skill: String,
    kind: String,
    successful: bool,
    source_attempt_id: Uuid,
    idempotency_key: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelpRequest {
    milestone_id: Uuid,
    skill: String,
    idempotency_key: Uuid,
}

fn skill_ok(skill: &str) -> bool {
    !skill.is_empty()
        && skill.len() <= 32
        && skill
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+#._-".contains(c))
}
fn initial_level(policy: &str) -> i16 {
    match policy {
        "guided_ai" => 7,
        "socratic_ai" => 6,
        "documentation_navigator" => 5,
        "curated_documentation" => 4,
        "cheat_sheet_only" => 3,
        "independent" => 2,
        _ => 1,
    }
}
fn mode_for_level(level: i16) -> &'static str {
    match level {
        7 => "guided_ai",
        6 => "socratic_ai",
        5 => "documentation_navigator",
        4 => "curated_documentation",
        3 => "cheat_sheet_only",
        2 => "independent",
        _ => "transfer_challenge",
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
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((event_kind, saved_project, milestone, skill, successful, source, previous, resulting, metadata)) = sqlx::query_as::<_,(String,Option<Uuid>,Option<Uuid>,Option<String>,Option<bool>,Option<Uuid>,Option<i16>,Option<i16>,SqlJson<Value>)>(
        "SELECT event_kind,project_id,milestone_id,skill,successful,source_attempt_id,previous_level,resulting_level,metadata FROM project_learning_events WHERE user_id=$1 AND idempotency_key=$2",
    ).bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await? {
        if event_kind != "assistance_evidence" || saved_project != Some(project) || milestone != Some(request.milestone_id)
            || skill.as_deref()!=Some(&request.skill) || successful!=Some(request.successful) || source!=Some(request.source_attempt_id)
            || metadata.0["kind"]!=request.kind {
            return Err(ProjectError::Conflict("같은 멱등키에 다른 도움 근거를 사용할 수 없습니다"));
        }
        tx.commit().await?;
        return Ok(Json(json!({"previous_level":previous,"level":resulting,"mode":resulting.map(mode_for_level),"rule_version":"project-learning-v1","source_attempt_id":source})));
    }
    let (policy, required_activity, required_skill): (String, Uuid, String) = sqlx::query_as("SELECT i.assistance_policy,m.required_activity_id,m.required_skill FROM learner_projects p JOIN project_ideas i ON i.id=p.idea_id JOIN project_milestones m ON m.project_id=p.id AND m.user_id=p.user_id WHERE p.id=$1 AND p.user_id=$2 AND m.id=$3")
        .bind(project).bind(user).bind(request.milestone_id).fetch_optional(&mut *tx).await?.ok_or(ProjectError::NotFound)?;
    if request.skill != required_skill {
        return Err(ProjectError::Invalid(
            "마일스톤에 지정된 교육과정 역량만 도움 근거로 사용할 수 있습니다",
        ));
    }
    let (attempt_activity, attempt_skill, passed, mastery_class): (Uuid, String, bool, Option<String>) = sqlx::query_as(
        "SELECT attempt.activity_id,axis.required_skill,attempt.passed,attempt.mastery_class FROM activity_attempts attempt JOIN learning_activity_axes axis ON axis.activity_id=attempt.activity_id WHERE attempt.id=$1 AND attempt.user_id=$2",
    )
    .bind(request.source_attempt_id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ProjectError::Invalid(
        "실제 학습 시도 영수증을 찾을 수 없습니다",
    ))?;
    if attempt_activity != required_activity || attempt_skill != required_skill {
        return Err(ProjectError::Invalid(
            "이 마일스톤에 지정된 학습 활동 시도만 근거로 사용할 수 있습니다",
        ));
    }
    if request.kind == "mastery"
        && (!request.successful
            || !passed
            || !matches!(mastery_class.as_deref(), Some("independent" | "explained")))
    {
        return Err(ProjectError::Invalid(
            "독립 또는 설명 완료 시도만 도움 감소 근거로 사용할 수 있습니다",
        ));
    }
    if request.kind == "attempt" && request.successful != passed {
        return Err(ProjectError::Invalid(
            "시도 성공 여부가 서버 영수증과 다릅니다",
        ));
    }
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_learning_events WHERE source_attempt_id=$1)",
    )
    .bind(request.source_attempt_id)
    .fetch_one(&mut *tx)
    .await?;
    if used {
        return Err(ProjectError::Conflict(
            "학습 시도 영수증은 한 번만 사용할 수 있습니다",
        ));
    }
    let initial = initial_level(&policy);
    sqlx::query("INSERT INTO project_assistance_states (project_id,milestone_id,user_id,skill,level,mode) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(project).bind(request.milestone_id).bind(user).bind(&request.skill).bind(initial).bind(mode_for_level(initial)).execute(&mut *tx).await?;
    let (previous,failures):(i16,i16)=sqlx::query_as("SELECT level,consecutive_failures FROM project_assistance_states WHERE project_id=$1 AND milestone_id=$2 AND user_id=$3 AND skill=$4 FOR UPDATE")
        .bind(project).bind(request.milestone_id).bind(user).bind(&request.skill).fetch_one(&mut *tx).await?;
    let (level, next_failures) = if request.kind == "mastery" {
        (previous.saturating_sub(1).max(1), 0)
    } else if request.successful {
        (previous, 0)
    } else if failures >= 1 {
        ((previous + 1).min(7), 0)
    } else {
        (previous, failures + 1)
    };
    sqlx::query("UPDATE project_assistance_states SET level=$5,mode=$6,consecutive_failures=$7,updated_at=now() WHERE project_id=$1 AND milestone_id=$2 AND user_id=$3 AND skill=$4")
        .bind(project).bind(request.milestone_id).bind(user).bind(&request.skill).bind(level).bind(mode_for_level(level)).bind(next_failures).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,milestone_id,event_kind,skill,successful,previous_level,resulting_level,idempotency_key,source_attempt_id,metadata) VALUES ($1,$2,$3,$4,'assistance_evidence',$5,$6,$7,$8,$9,$10,$11)")
        .bind(Uuid::now_v7()).bind(user).bind(project).bind(request.milestone_id).bind(&request.skill).bind(request.successful).bind(previous).bind(level).bind(request.idempotency_key).bind(request.source_attempt_id).bind(json!({"kind":request.kind})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"previous_level":previous,"level":level,"mode":mode_for_level(level),"rule_version":"project-learning-v1","source_attempt_id":request.source_attempt_id}),
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
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((event_kind,saved_project,milestone,skill,level))=sqlx::query_as::<_,(String,Option<Uuid>,Option<Uuid>,Option<String>,Option<i16>)>(
        "SELECT event_kind,project_id,milestone_id,skill,resulting_level FROM project_learning_events WHERE user_id=$1 AND idempotency_key=$2",
    ).bind(user).bind(request.idempotency_key).fetch_optional(&mut *tx).await? {
        if event_kind!="assistance_requested"||saved_project!=Some(project)||milestone!=Some(request.milestone_id)||skill.as_deref()!=Some(&request.skill){
            return Err(ProjectError::Conflict("같은 멱등키에 다른 도움 요청을 사용할 수 없습니다"));
        }
        let level=level.ok_or(ProjectError::Conflict("도움 요청 기록이 올바르지 않습니다"))?;
        tx.commit().await?;
        return Ok(Json(json!({"level":level,"mode":mode_for_level(level),"content":help_content(level),"provider_used":false,"rule_version":"project-learning-v1"})));
    }
    let (policy,required_skill):(String,String)=sqlx::query_as("SELECT i.assistance_policy,m.required_skill FROM learner_projects p JOIN project_ideas i ON i.id=p.idea_id JOIN project_milestones m ON m.project_id=p.id AND m.user_id=p.user_id WHERE p.id=$1 AND p.user_id=$2 AND m.id=$3")
        .bind(project).bind(user).bind(request.milestone_id).fetch_optional(&mut *tx).await?.ok_or(ProjectError::NotFound)?;
    if request.skill != required_skill {
        return Err(ProjectError::Invalid(
            "마일스톤에 지정된 교육과정 역량만 도움을 요청할 수 있습니다",
        ));
    }
    let level:Option<i16>=sqlx::query_scalar("SELECT level FROM project_assistance_states WHERE project_id=$1 AND milestone_id=$2 AND user_id=$3 AND skill=$4")
        .bind(project).bind(request.milestone_id).bind(user).bind(&request.skill).fetch_optional(&mut *tx).await?;
    let level = level.unwrap_or(initial_level(&policy));
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,milestone_id,event_kind,skill,resulting_level,idempotency_key,metadata) VALUES ($1,$2,$3,$4,'assistance_requested',$5,$6,$7,$8)")
        .bind(Uuid::now_v7()).bind(user).bind(project).bind(request.milestone_id).bind(&request.skill).bind(level).bind(request.idempotency_key).bind(json!({"provider_used":false})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"level":level,"mode":mode_for_level(level),"content":help_content(level),"provider_used":false,"rule_version":"project-learning-v1"}),
    ))
}

fn help_content(level: i16) -> &'static str {
    match level {
        1 => "이전 과제와 실질적으로 다른 맥락에서 다시 구현하고 새 독립 시도 영수증을 제출하세요.",
        2 => "요구사항, 실행 환경, 테스트만 보고 독립적으로 구현하세요.",
        3 => "치트 시트: cargo test · npm test · 입력 경계 확인 · 오류 메시지의 첫 원인부터 추적.",
        4 => "승인된 공식 문서의 관련 절만 확인하세요.",
        5 => "공식 문서에서 API 이름, 버전, 오류 계약을 먼저 찾으세요.",
        6 => "어떤 입력과 실패 조건부터 검증하면 좋을까요?",
        _ => "목표를 작은 단계로 나누고 다음 한 단계만 구현하세요.",
    }
}
