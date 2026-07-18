use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

type ReceiptView = (String, String, String, String, i32, Value, String, Value);

#[derive(Debug)]
pub enum ReviewError {
    Auth(AuthError),
    Invalid(&'static str),
    Forbidden,
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}
impl From<AuthError> for ReviewError {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}
impl From<sqlx::Error> for ReviewError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
impl IntoResponse for ReviewError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "content_review_forbidden",
                "검토 권한이 없습니다",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "content_review_not_found",
                "검토 대상을 찾을 수 없습니다",
            ),
            Self::Conflict(message) => (StatusCode::CONFLICT, "content_review_conflict", message),
            Self::Database(error) => {
                tracing::error!(%error, "콘텐츠 검토 처리 실패");
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

async fn user(state: &AppState, headers: &HeaderMap, csrf: bool) -> Result<Uuid, ReviewError> {
    let id = if csrf {
        crate::auth::authenticated_user_id_with_csrf(state, headers).await?
    } else {
        crate::auth::authenticated_user_id(state, headers).await?
    };
    crate::auth::require_current_policy(state, id).await?;
    Ok(id)
}
async fn require(state: &AppState, id: Uuid, capability: &str) -> Result<(), ReviewError> {
    if !crate::governance::has_capability(state.pool(), id, capability).await? {
        return Err(ReviewError::Forbidden);
    }
    Ok(())
}
fn hex32(raw: &str) -> Result<Vec<u8>, ReviewError> {
    if raw.len() != 64 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ReviewError::Invalid("SHA-256 해시를 확인해 주세요"));
    }
    (0..64)
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&raw[i..i + 2], 16)
                .map_err(|_| ReviewError::Invalid("SHA-256 해시를 확인해 주세요"))
        })
        .collect()
}
fn audit_query() -> &'static str {
    "INSERT INTO audit_events (actor_user_id,action,target_type,target_id,metadata) VALUES ($1,$2,'content_review',$3,$4)"
}
fn receipt_hash<T: Serialize>(input: &T) -> Result<Vec<u8>, ReviewError> {
    Ok(Sha256::digest(
        serde_json::to_vec(input)
            .map_err(|_| ReviewError::Invalid("영수증을 직렬화할 수 없습니다"))?,
    )
    .to_vec())
}
async fn can_review(state: &AppState, actor: Uuid) -> Result<bool, ReviewError> {
    for capability in [
        "content.review.ai",
        "content.review.human",
        "content.review.legal",
        "content.retire",
    ] {
        if crate::governance::has_capability(state.pool(), actor, capability).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    artifact_id: Uuid,
    impact: String,
    idempotency_key: Uuid,
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateInput>,
) -> Result<(StatusCode, Json<Value>), ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.generate").await?;
    if !matches!(input.impact.as_str(), "standard" | "high" | "novel") {
        return Err(ReviewError::Invalid("영향 수준을 확인해 주세요"));
    }
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("{actor}:{}", input.idempotency_key))
        .execute(&mut *tx)
        .await?;
    if let Some((id,state_name,artifact_id,impact))=sqlx::query_as::<_,(Uuid,String,Uuid,String)>("SELECT id,state,artifact_id,impact FROM content_review_items WHERE author_user_id=$1 AND create_idempotency_key=$2").bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await? {
        if artifact_id!=input.artifact_id||impact!=input.impact{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 검토 요청을 사용할 수 없습니다"));}
        tx.commit().await?; return Ok((StatusCode::OK,Json(json!({"id":id,"state":state_name}))));
    }
    let artifact=sqlx::query_as::<_,(Vec<u8>,Uuid)>("SELECT artifact.content_hash,job.created_by FROM content_artifacts artifact JOIN content_generation_jobs job ON job.id=artifact.job_id WHERE artifact.id=$1 AND artifact.artifact_kind='candidate'")
        .bind(input.artifact_id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if artifact.1 != actor {
        return Err(ReviewError::Forbidden);
    }
    let id:Uuid=sqlx::query_scalar("INSERT INTO content_review_items (artifact_id,artifact_hash,author_user_id,impact,create_idempotency_key) VALUES ($1,$2,$3,$4,$5) RETURNING id")
        .bind(input.artifact_id).bind(&artifact.0).bind(actor).bind(&input.impact).bind(input.idempotency_key).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key) VALUES ($1,1,$2,$3,$4,$5)")
        .bind(id).bind(input.artifact_id).bind(&artifact.0).bind(actor).bind(input.idempotency_key).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.created")
        .bind(id.to_string())
        .bind(json!({"revision":1,"artifact_id":input.artifact_id}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"state":"ai_review_pending","revision":1})),
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiReceipt {
    kind: String,
    provider: String,
    model: String,
    prompt_version: String,
    role_identifier: String,
    prompt_hash: String,
    review_seed: i64,
    input_hash: String,
    output_hash: String,
    latency_ms: i32,
    usage: Value,
    outcome: String,
    findings: Value,
    idempotency_key: Uuid,
}
pub async fn ai_receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AiReceipt>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.review.ai").await?;
    if !matches!(
        input.kind.as_str(),
        "specification_pedagogy" | "solution_judge" | "adversarial_rights"
    ) || !matches!(input.outcome.as_str(), "pass" | "fail")
        || input.provider.trim().is_empty()
        || input.model.trim().is_empty()
        || input.prompt_version.trim().len() < 3
        || input.role_identifier.trim().len() < 3
        || input.latency_ms < 1
        || !input.usage.is_object()
        || !input.findings.is_array()
        || (input.outcome == "fail" && input.findings.as_array().is_some_and(Vec::is_empty))
    {
        return Err(ReviewError::Invalid("AI 검토 영수증을 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let input_hash = hex32(&input.input_hash)?;
    let output_hash = hex32(&input.output_hash)?;
    let prompt_hash = hex32(&input.prompt_hash)?;
    let mut tx = state.pool().begin().await?;
    let (revision,expected,author,state_name):(i32,Vec<u8>,Uuid,String)=sqlx::query_as("SELECT revision_number,artifact_hash,author_user_id,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if actor == author {
        return Err(ReviewError::Forbidden);
    }
    if let Some((old_revision,old_hash))=sqlx::query_as::<_,(i32,Vec<u8>)>("SELECT revision_number,receipt_hash FROM content_ai_review_receipts WHERE review_item_id=$1 AND operator_user_id=$2 AND idempotency_key=$3")
        .bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await? {
        if old_revision==revision&&old_hash==canonical_hash { tx.commit().await?; return Ok(Json(json!({"state":state_name,"replayed":true}))); }
        return Err(ReviewError::Conflict("같은 멱등성 키에 다른 AI 검토 영수증을 사용할 수 없습니다"));
    }
    if state_name != "ai_review_pending" {
        return Err(ReviewError::Conflict(
            "현재 리비전은 AI 검토 단계가 아닙니다",
        ));
    }
    if input_hash != expected {
        return Err(ReviewError::Conflict(
            "현재 불변 아티팩트 해시와 일치하지 않습니다",
        ));
    }
    if let Some((old_hash,old_outcome))=sqlx::query_as::<_,(Vec<u8>,String)>("SELECT output_hash,outcome FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND review_kind=$3")
        .bind(id).bind(revision).bind(&input.kind).fetch_optional(&mut *tx).await? {
        if old_hash==output_hash && old_outcome==input.outcome { tx.commit().await?; return Ok(Json(json!({"state":state_name,"replayed":true}))); }
        return Err(ReviewError::Conflict("같은 검토 종류의 영수증이 이미 기록됐습니다"));
    }
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND output_hash=$3)").bind(id).bind(revision).bind(&output_hash).fetch_one(&mut *tx).await? { return Err(ReviewError::Conflict("같은 모델 응답은 독립 검토로 재사용할 수 없습니다")); }
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND provider=$3 AND model=$4 AND prompt_hash=$5 AND review_seed=$6)").bind(id).bind(revision).bind(input.provider.trim()).bind(input.model.trim()).bind(&prompt_hash).bind(input.review_seed).fetch_one(&mut *tx).await? { return Err(ReviewError::Conflict("같은 모델 검토는 역할 프롬프트 또는 시드를 분리해야 합니다")); }
    sqlx::query("INSERT INTO content_ai_review_receipts (review_item_id,revision_number,review_kind,operator_user_id,provider,model,prompt_version,role_identifier,prompt_hash,review_seed,input_hash,output_hash,latency_ms,usage,outcome,findings,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)")
        .bind(id).bind(revision).bind(&input.kind).bind(actor).bind(input.provider.trim()).bind(input.model.trim()).bind(input.prompt_version.trim()).bind(input.role_identifier.trim()).bind(prompt_hash).bind(input.review_seed).bind(input_hash).bind(output_hash).bind(input.latency_ms).bind(&input.usage).bind(&input.outcome).bind(&input.findings).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
    let next = if input.outcome == "fail" {
        "changes_requested"
    } else {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND outcome='pass'").bind(id).bind(revision).fetch_one(&mut *tx).await?;
        if count == 3 {
            "human_review_pending"
        } else {
            "ai_review_pending"
        }
    };
    sqlx::query("UPDATE content_review_items SET state=$2,updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.ai_recorded")
        .bind(id.to_string())
        .bind(json!({"revision":revision,"kind":input.kind,"outcome":input.outcome,"state":next}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next,"revision":revision})))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanInput {
    decision: String,
    note: String,
    idempotency_key: Uuid,
}
pub async fn human(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<HumanInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.review.human").await?;
    if !matches!(
        input.decision.as_str(),
        "approve" | "changes_requested" | "reject"
    ) || !(3..=2000).contains(&input.note.trim().chars().count())
    {
        return Err(ReviewError::Invalid("사람 검토 결정을 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let mut tx = state.pool().begin().await?;
    let (revision,author,state_name):(i32,Uuid,String)=sqlx::query_as("SELECT revision_number,author_user_id,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if actor == author {
        return Err(ReviewError::Forbidden);
    }
    if let Some((old_revision,old_hash))=sqlx::query_as::<_,(i32,Vec<u8>)>("SELECT revision_number,receipt_hash FROM content_human_review_decisions WHERE review_item_id=$1 AND reviewer_user_id=$2 AND idempotency_key=$3").bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await? { if old_revision!=revision||old_hash!=canonical_hash{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 사람 검토 결정을 사용할 수 없습니다"));}tx.commit().await?; return Ok(Json(json!({"state":state_name,"decision":input.decision,"replayed":true}))); }
    if state_name != "human_review_pending" {
        return Err(ReviewError::Conflict(
            "세 AI 검토 통과 뒤에만 사람 검토할 수 있습니다",
        ));
    }
    let next = match input.decision.as_str() {
        "approve" => "rights_review_pending",
        "changes_requested" => "changes_requested",
        _ => "rejected",
    };
    sqlx::query("INSERT INTO content_human_review_decisions (review_item_id,revision_number,reviewer_user_id,decision,note,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(revision).bind(actor).bind(&input.decision).bind(input.note.trim()).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
    sqlx::query("UPDATE content_review_items SET state=$2,human_reviewer_id=$3,updated_at=now() WHERE id=$1").bind(id).bind(next).bind(actor).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.human_decided")
        .bind(id.to_string())
        .bind(json!({"revision":revision,"decision":input.decision}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next})))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RightsInput {
    decision: String,
    basis: String,
    evidence: String,
    commercial_use_allowed: bool,
    redistribution_allowed: bool,
    provider_terms_version: String,
    idempotency_key: Uuid,
}
pub async fn rights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RightsInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.review.legal").await?;
    if !matches!(input.decision.as_str(), "approve" | "reject")
        || !matches!(
            input.basis.as_str(),
            "original" | "licensed" | "public_domain" | "contract"
        )
        || input.evidence.trim().chars().count() < 3
        || input.provider_terms_version.trim().chars().count() < 3
    {
        return Err(ReviewError::Invalid("권리 검토 근거를 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let mut tx = state.pool().begin().await?;
    let (revision,author,human,state_name,impact):(i32,Uuid,Option<Uuid>,String,String)=sqlx::query_as("SELECT revision_number,author_user_id,human_reviewer_id,state,impact FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if actor == author || human == Some(actor) {
        return Err(ReviewError::Forbidden);
    }
    if let Some((old_revision,old_hash))=sqlx::query_as::<_,(i32,Vec<u8>)>("SELECT revision_number,receipt_hash FROM content_rights_review_receipts WHERE review_item_id=$1 AND reviewer_user_id=$2 AND idempotency_key=$3").bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await?{if old_revision!=revision||old_hash!=canonical_hash{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 권리 검토 결정을 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"state":state_name,"decision":input.decision,"replayed":true})));}
    if state_name != "rights_review_pending" {
        return Err(ReviewError::Conflict(
            "사람 승인 뒤에만 권리 검토할 수 있습니다",
        ));
    }
    let approved =
        input.decision == "approve" && input.commercial_use_allowed && input.redistribution_allowed;
    let next = if approved && impact == "standard" {
        "approved"
    } else if approved {
        "pilot_pending"
    } else {
        "changes_requested"
    };
    sqlx::query("INSERT INTO content_rights_review_receipts (review_item_id,revision_number,reviewer_user_id,decision,basis,evidence,commercial_use_allowed,redistribution_allowed,provider_terms_version,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(id).bind(revision).bind(actor).bind(&input.decision).bind(&input.basis).bind(input.evidence.trim()).bind(input.commercial_use_allowed).bind(input.redistribution_allowed).bind(input.provider_terms_version.trim()).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
    sqlx::query("UPDATE content_review_items SET state=$2,rights_reviewer_id=$3,updated_at=now() WHERE id=$1").bind(id).bind(next).bind(actor).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.rights_decided")
        .bind(id.to_string())
        .bind(json!({"revision":revision,"decision":input.decision,"usable":approved}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next})))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotInput {
    cohort: String,
    #[serde(with = "time::serde::rfc3339")]
    started_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    ended_at: time::OffsetDateTime,
    participants: i32,
    completion_rate: f64,
    failure_rate: f64,
    report_count: i32,
    rollback_ready: bool,
    decision: String,
    note: String,
    idempotency_key: Uuid,
}
pub async fn pilot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<PilotInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.review.human").await?;
    if input.cohort.trim().chars().count() < 3
        || input.ended_at <= input.started_at
        || input.participants < 5
        || !(0.0..=1.0).contains(&input.completion_rate)
        || !(0.0..=1.0).contains(&input.failure_rate)
        || input.report_count < 0
        || !matches!(input.decision.as_str(), "pass" | "fail")
        || input.note.trim().chars().count() < 3
    {
        return Err(ReviewError::Invalid("파일럿 증거를 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let mut tx = state.pool().begin().await?;
    let (revision,human,state_name):(i32,Option<Uuid>,String)=sqlx::query_as("SELECT revision_number,human_reviewer_id,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if human != Some(actor) {
        return Err(ReviewError::Forbidden);
    }
    if let Some((old_revision,old_hash))=sqlx::query_as::<_,(i32,Vec<u8>)>("SELECT revision_number,receipt_hash FROM content_pilot_receipts WHERE review_item_id=$1 AND reviewer_user_id=$2 AND idempotency_key=$3").bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await?{if old_revision!=revision||old_hash!=canonical_hash{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 파일럿 결과를 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"state":state_name,"decision":input.decision,"replayed":true})));}
    if state_name != "pilot_pending" {
        return Err(ReviewError::Conflict(
            "권리 승인 뒤에만 파일럿을 기록할 수 있습니다",
        ));
    }
    let passed = input.decision == "pass"
        && input.rollback_ready
        && input.completion_rate >= 0.6
        && input.failure_rate <= 0.3;
    let next = if passed {
        "approved"
    } else {
        "changes_requested"
    };
    sqlx::query("INSERT INTO content_pilot_receipts (review_item_id,revision_number,reviewer_user_id,cohort,started_at,ended_at,participants,completion_rate,failure_rate,report_count,rollback_ready,decision,note,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)").bind(id).bind(revision).bind(actor).bind(input.cohort.trim()).bind(input.started_at).bind(input.ended_at).bind(input.participants).bind(input.completion_rate).bind(input.failure_rate).bind(input.report_count).bind(input.rollback_ready).bind(&input.decision).bind(input.note.trim()).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
    sqlx::query("UPDATE content_review_items SET state=$2,updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.pilot_decided")
        .bind(id.to_string())
        .bind(json!({"revision":revision,"decision":input.decision,"passed":passed}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyInput {
    idempotency_key: Uuid,
}
pub async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<KeyInput>,
) -> Result<Json<Value>, ReviewError> {
    transition_publication(
        &state,
        &headers,
        id,
        "published",
        "content.published",
        Some(input.idempotency_key),
        None,
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasonInput {
    reason: String,
    idempotency_key: Uuid,
}
pub async fn unpublish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReasonInput>,
) -> Result<Json<Value>, ReviewError> {
    if input.reason.trim().chars().count() < 3 {
        return Err(ReviewError::Invalid("게시 해제 사유를 입력해 주세요"));
    }
    transition_publication(
        &state,
        &headers,
        id,
        "unpublished",
        "content.unpublished",
        Some(input.idempotency_key),
        Some(input.reason),
    )
    .await
}
pub async fn removal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReasonInput>,
) -> Result<Json<Value>, ReviewError> {
    if input.reason.trim().chars().count() < 3 {
        return Err(ReviewError::Invalid("제거 사유를 입력해 주세요"));
    }
    transition_publication(
        &state,
        &headers,
        id,
        "removal_pending",
        "content.removal_pending",
        Some(input.idempotency_key),
        Some(input.reason),
    )
    .await
}
async fn transition_publication(
    state: &AppState,
    headers: &HeaderMap,
    id: Uuid,
    next: &str,
    action: &str,
    key: Option<Uuid>,
    reason: Option<String>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(state, headers, true).await?;
    require(
        state,
        actor,
        if next == "published" {
            "content.publish"
        } else {
            "content.retire"
        },
    )
    .await?;
    let mut tx = state.pool().begin().await?;
    let (author,human,rights,current):(Uuid,Option<Uuid>,Option<Uuid>,String)=sqlx::query_as("SELECT author_user_id,human_reviewer_id,rights_reviewer_id,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if let Some(old_reason)=sqlx::query_scalar::<_,Option<String>>("SELECT metadata->>'reason' FROM audit_events WHERE actor_user_id=$1 AND action=$2 AND target_type='content_review' AND target_id=$3 AND metadata->>'idempotency_key'=$4").bind(actor).bind(action).bind(id.to_string()).bind(key.unwrap().to_string()).fetch_optional(&mut *tx).await?{if old_reason!=reason{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 게시 전이를 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"state":current,"replayed":true})));}
    if actor == author || human == Some(actor) || rights == Some(actor) {
        return Err(ReviewError::Forbidden);
    }
    let allowed = if next == "published" {
        current == "approved"
    } else {
        matches!(current.as_str(), "published" | "approved" | "unpublished")
    };
    if !allowed {
        return Err(ReviewError::Conflict(
            "현재 승인 상태에서는 해당 전이를 수행할 수 없습니다",
        ));
    }
    sqlx::query("UPDATE content_review_items SET state=$2,published_by=CASE WHEN $2='published' THEN $3 ELSE published_by END,published_at=CASE WHEN $2='published' THEN now() ELSE published_at END,updated_at=now() WHERE id=$1").bind(id).bind(next).bind(actor).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind(action)
        .bind(id.to_string())
        .bind(json!({"idempotency_key":key,"reason":reason}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviseInput {
    artifact_id: Uuid,
    idempotency_key: Uuid,
}
pub async fn revise(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReviseInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.generate").await?;
    let mut tx = state.pool().begin().await?;
    let (author,revision,old_hash,state_name):(Uuid,i32,Vec<u8>,String)=sqlx::query_as("SELECT author_user_id,revision_number,artifact_hash,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if actor != author {
        return Err(ReviewError::Forbidden);
    }
    if let Some(existing)=sqlx::query_scalar::<_,i32>("SELECT revision_number FROM content_review_revisions WHERE review_item_id=$1 AND changed_by=$2 AND idempotency_key=$3").bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await?{tx.commit().await?;return Ok(Json(json!({"state":state_name,"revision":existing,"replayed":true})));}
    if state_name != "changes_requested" && state_name != "unpublished" {
        return Err(ReviewError::Conflict(
            "변경 요청 또는 게시 해제 상태에서만 새 리비전을 만들 수 있습니다",
        ));
    }
    let hash:Vec<u8>=sqlx::query_scalar("SELECT artifact.content_hash FROM content_artifacts artifact JOIN content_generation_jobs job ON job.id=artifact.job_id WHERE artifact.id=$1 AND artifact.artifact_kind='candidate' AND job.created_by=$2").bind(input.artifact_id).bind(actor).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if hash == old_hash {
        return Err(ReviewError::Conflict(
            "새 리비전은 다른 아티팩트 해시가 필요합니다",
        ));
    }
    let next = revision + 1;
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key) VALUES ($1,$2,$3,$4,$5,$6)").bind(id).bind(next).bind(input.artifact_id).bind(&hash).bind(actor).bind(input.idempotency_key).execute(&mut *tx).await?;
    sqlx::query("UPDATE content_review_items SET artifact_id=$2,artifact_hash=$3,revision_number=$4,state='ai_review_pending',human_reviewer_id=NULL,rights_reviewer_id=NULL,published_by=NULL,published_at=NULL,updated_at=now() WHERE id=$1").bind(id).bind(input.artifact_id).bind(hash).bind(next).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind("content.review.revised")
        .bind(id.to_string())
        .bind(json!({"revision":next,"artifact_id":input.artifact_id}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":"ai_review_pending","revision":next})))
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, false).await?;
    let privileged = can_review(&state, actor).await?;
    let row=sqlx::query_as::<_,(Uuid,Uuid,i32,String,String,Value)>("SELECT item.author_user_id,item.artifact_id,item.revision_number,item.state,encode(item.artifact_hash,'hex'),artifact.payload FROM content_review_items item JOIN content_artifacts artifact ON artifact.id=item.artifact_id WHERE item.id=$1").bind(id).fetch_optional(state.pool()).await?.ok_or(ReviewError::NotFound)?;
    if row.0 != actor && !privileged {
        return Err(ReviewError::NotFound);
    }
    let receipts:Vec<ReceiptView>=sqlx::query_as("SELECT review_kind,provider,model,prompt_version,latency_ms,usage,outcome,findings FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 ORDER BY review_kind").bind(id).bind(row.2).fetch_all(state.pool()).await?;
    Ok(Json(
        json!({"id":id,"artifact_id":row.1,"revision":row.2,"state":row.3,"artifact_hash":row.4,"artifact":row.5,"ai_receipts":receipts.into_iter().map(|r|json!({"kind":r.0,"provider":r.1,"model":r.2,"prompt_version":r.3,"latency_ms":r.4,"usage":r.5,"outcome":r.6,"findings":r.7})).collect::<Vec<_>>()}),
    ))
}
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, false).await?;
    let privileged = can_review(&state, actor).await?;
    let rows:Vec<(Uuid,i32,String,String,time::OffsetDateTime)>=sqlx::query_as("SELECT id,revision_number,state,encode(artifact_hash,'hex'),updated_at FROM content_review_items WHERE author_user_id=$1 OR $2 ORDER BY updated_at DESC LIMIT 100").bind(actor).bind(privileged).fetch_all(state.pool()).await?;
    Ok(Json(
        json!({"reviews":rows.into_iter().map(|r|json!({"id":r.0,"revision":r.1,"state":r.2,"artifact_hash":r.3,"updated_at":r.4})).collect::<Vec<_>>()}),
    ))
}
pub async fn public(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ReviewError> {
    let row=sqlx::query_as::<_,(i32,String,Value,String)>(r#"SELECT item.revision_number,job.content_type,
    jsonb_strip_nulls(CASE job.content_type
      WHEN 'algorithm_problem' THEN jsonb_build_object('title',artifact.payload->'title','statement',artifact.payload->'statement','examples',artifact.payload->'examples')
      WHEN 'code_reading' THEN jsonb_build_object('title',artifact.payload->'title','code',artifact.payload->'code','question',artifact.payload->'question')
      WHEN 'debugging' THEN jsonb_build_object('title',artifact.payload->'title','buggy_code',artifact.payload->'buggy_code','question',artifact.payload->'question')
      WHEN 'documentation_lesson' THEN jsonb_build_object('title',artifact.payload->'title','lesson',artifact.payload->'lesson','learning_objective',artifact.payload->'learning_objective')
      WHEN 'implementation_task' THEN jsonb_build_object('title',artifact.payload->'title','requirements',artifact.payload->'requirements')
    END),encode(item.artifact_hash,'hex')
    FROM content_review_items item JOIN content_artifacts artifact ON artifact.id=item.artifact_id JOIN content_generation_jobs job ON job.id=artifact.job_id
    WHERE item.id=$1 AND item.state='published'"#).bind(id).fetch_optional(state.pool()).await?.ok_or(ReviewError::NotFound)?;
    Ok(Json(
        json!({"id":id,"revision":row.0,"content_type":row.1,"content":row.2,"artifact_hash":row.3}),
    ))
}
