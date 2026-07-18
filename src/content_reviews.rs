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
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn valid_review_context(value: &Value, kind: &str) -> bool {
    value.as_object().is_some_and(|map| {
        map.len() == 3
            && map
                .get("artifact_hash")
                .and_then(Value::as_str)
                .is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            && map
                .get("policy")
                .and_then(Value::as_str)
                .is_some_and(|text| (3..=2000).contains(&text.chars().count()))
            && map.get("kind").and_then(Value::as_str) == Some(kind)
    })
}
fn valid_review_response(value: &Value, outcome: &str) -> bool {
    value.as_object().is_some_and(|map| {
        map.len() == 3
            && map
                .get("summary")
                .and_then(Value::as_str)
                .is_some_and(|text| (3..=4000).contains(&text.chars().count()))
            && map.get("outcome").and_then(Value::as_str) == Some(outcome)
            && map
                .get("scores")
                .and_then(Value::as_object)
                .is_some_and(|scores| {
                    scores.len() <= 20
                        && scores.iter().all(|(key, value)| {
                            (1..=80).contains(&key.chars().count())
                                && value
                                    .as_f64()
                                    .is_some_and(|score| (0.0..=1.0).contains(&score))
                        })
                })
    })
}
fn valid_usage(value: &Value) -> bool {
    value.as_object().is_some_and(|usage| {
        !usage.is_empty()
            && usage.len() <= 5
            && usage.iter().all(|(key, value)| {
                let maximum = match key.as_str() {
                    "input_tokens" | "output_tokens" => 1_000_000_000,
                    "total_tokens" => 2_000_000_000,
                    "cost_microunits" => 1_000_000_000_000,
                    "local_compute_ms" => 3_600_000,
                    _ => return false,
                };
                value.as_u64().is_some_and(|amount| amount <= maximum)
            })
    })
}
fn valid_findings(findings: &Value, outcome: &str) -> bool {
    findings.as_array().is_some_and(|items| {
        (outcome != "fail" || !items.is_empty())
            && items.iter().all(|item| {
                item.as_object().is_some_and(|finding| {
                    finding.len() == 4
                        && finding
                            .get("severity")
                            .and_then(Value::as_str)
                            .is_some_and(|value| {
                                matches!(value, "low" | "medium" | "high" | "critical")
                            })
                        && finding
                            .get("evidence")
                            .and_then(Value::as_str)
                            .is_some_and(|value| (1..=2000).contains(&value.chars().count()))
                        && finding
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(|value| (3..=2000).contains(&value.chars().count()))
                        && finding
                            .get("proposed_fix")
                            .and_then(Value::as_str)
                            .is_some_and(|value| (3..=2000).contains(&value.chars().count()))
                })
            })
    })
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

#[derive(Deserialize, Serialize)]
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("content-review-artifact:{}", input.artifact_id))
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM content_review_revisions WHERE artifact_id=$1)",
    )
    .bind(input.artifact_id)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(ReviewError::Conflict(
            "같은 아티팩트의 검토가 이미 존재합니다",
        ));
    }
    let id:Uuid=sqlx::query_scalar("INSERT INTO content_review_items (artifact_id,artifact_hash,author_user_id,impact,create_idempotency_key) VALUES ($1,$2,$3,$4,$5) RETURNING id")
        .bind(input.artifact_id).bind(&artifact.0).bind(actor).bind(&input.impact).bind(input.idempotency_key).fetch_one(&mut *tx).await?;
    let create_hash = receipt_hash(&input)?;
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key,request_hash) VALUES ($1,1,$2,$3,$4,$5,$6)")
        .bind(id).bind(input.artifact_id).bind(&artifact.0).bind(actor).bind(input.idempotency_key).bind(create_hash).execute(&mut *tx).await?;
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
    review_seed: i64,
    review_context: Value,
    review_response: Value,
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
    let expected_contract = match input.kind.as_str() {
        "specification_pedagogy" => ("specification-pedagogy-reviewer", "spec-pedagogy-v1"),
        "solution_judge" => ("solution-judge-reviewer", "solution-judge-v1"),
        "adversarial_rights" => ("adversarial-rights-reviewer", "adversarial-rights-v1"),
        _ => return Err(ReviewError::Invalid("AI 검토 종류를 확인해 주세요")),
    };
    let serialized_size = serde_json::to_vec(&input)
        .map_err(|_| ReviewError::Invalid("AI 검토 영수증을 확인해 주세요"))?
        .len();
    if !matches!(input.outcome.as_str(), "pass" | "fail")
        || !(1..=120).contains(&input.provider.trim().chars().count())
        || !(1..=120).contains(&input.model.trim().chars().count())
        || input.prompt_version != expected_contract.1
        || input.role_identifier != expected_contract.0
        || input.latency_ms < 1
        || input.latency_ms > 3_600_000
        || !valid_usage(&input.usage)
        || !input.review_context.is_object()
        || !input.review_response.is_object()
        || !valid_review_context(&input.review_context, &input.kind)
        || !valid_review_response(&input.review_response, &input.outcome)
        || serialized_size > 1_048_576
        || !valid_findings(&input.findings, &input.outcome)
    {
        return Err(ReviewError::Invalid("AI 검토 영수증을 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let prompt_hash = receipt_hash(&json!({
        "kind": input.kind,
        "role_identifier": input.role_identifier,
        "prompt_version": input.prompt_version
    }))?;
    let context_hash = receipt_hash(&input.review_context)?;
    let output_hash = receipt_hash(&input.review_response)?;
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
    let expected_hex = hex(&expected);
    if input
        .review_context
        .get("artifact_hash")
        .and_then(Value::as_str)
        != Some(expected_hex.as_str())
    {
        return Err(ReviewError::Conflict(
            "현재 불변 아티팩트 해시와 일치하지 않습니다",
        ));
    }
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND review_kind=$3)").bind(id).bind(revision).bind(&input.kind).fetch_one(&mut *tx).await? {
        return Err(ReviewError::Conflict("같은 검토 종류의 영수증이 이미 기록됐습니다"));
    }
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND output_hash=$3)").bind(id).bind(revision).bind(&output_hash).fetch_one(&mut *tx).await? { return Err(ReviewError::Conflict("같은 모델 응답은 독립 검토로 재사용할 수 없습니다")); }
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 AND (review_seed=$3 OR context_hash=$4 OR prompt_hash=$5))").bind(id).bind(revision).bind(input.review_seed).bind(&context_hash).bind(&prompt_hash).fetch_one(&mut *tx).await? { return Err(ReviewError::Conflict("독립 AI 검토는 프롬프트·컨텍스트·시드를 분리해야 합니다")); }
    sqlx::query("INSERT INTO content_ai_review_receipts (review_item_id,revision_number,review_kind,operator_user_id,provider,model,prompt_version,role_identifier,prompt_hash,review_seed,review_context,context_hash,review_response,input_hash,output_hash,latency_ms,usage,outcome,findings,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)")
        .bind(id).bind(revision).bind(&input.kind).bind(actor).bind(input.provider.trim()).bind(input.model.trim()).bind(input.prompt_version.trim()).bind(input.role_identifier.trim()).bind(prompt_hash).bind(input.review_seed).bind(&input.review_context).bind(context_hash).bind(&input.review_response).bind(expected).bind(output_hash).bind(input.latency_ms).bind(&input.usage).bind(&input.outcome).bind(&input.findings).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
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
pub struct ProvenanceInput {
    origin_type: String,
    creator_or_provider: String,
    source_url: Option<String>,
    source_revision: String,
    license_basis: String,
    license_identifier: String,
    attribution: String,
    modification_status: String,
    commercial_use_allowed: bool,
    redistribution_allowed: bool,
    evidence_reference: String,
    evidence_hash: String,
    attachment_metadata: Value,
    legal_status: String,
    idempotency_key: Uuid,
}
type ProvenanceSourceRow = (i32, Vec<u8>, Uuid, String, Uuid, Uuid, Value, Vec<u8>);
pub async fn record_provenance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ProvenanceInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.generate").await?;
    if input.origin_type != "ai_generated"
        || input.license_basis != "provider_contract"
        || !matches!(
            input.modification_status.as_str(),
            "unmodified" | "modified" | "translated" | "generated"
        )
        || input.legal_status != "pending"
        || !(2..=300).contains(&input.creator_or_provider.trim().chars().count())
        || !(2..=300).contains(&input.source_revision.trim().chars().count())
        || !(2..=300).contains(&input.license_identifier.trim().chars().count())
        || !(2..=2000).contains(&input.attribution.trim().chars().count())
        || !(3..=1000).contains(&input.evidence_reference.trim().chars().count())
        || input.source_url.is_some()
        || !input.attachment_metadata.is_object()
    {
        return Err(ReviewError::Invalid(
            "출처와 권리 증거를 완전하게 입력해 주세요",
        ));
    }
    let evidence_hash = hex32(&input.evidence_hash)?;
    let mut tx = state.pool().begin().await?;
    let row: ProvenanceSourceRow=sqlx::query_as("SELECT item.revision_number,item.artifact_hash,item.author_user_id,item.state,artifact.job_id,artifact.attempt_id,attempt.provider_snapshot,attempt.request_hash FROM content_review_items item JOIN content_artifacts artifact ON artifact.id=item.artifact_id JOIN content_generation_attempts attempt ON attempt.id=artifact.attempt_id WHERE item.id=$1 FOR UPDATE OF item").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if row.2 != actor {
        return Err(ReviewError::Forbidden);
    }
    if matches!(row.3.as_str(), "published" | "removal_completed") {
        return Err(ReviewError::Conflict(
            "게시 또는 제거 완료 뒤에는 출처 기록을 바꿀 수 없습니다",
        ));
    }
    let provider_id = row
        .6
        .get("provider_id")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<Uuid>().ok())
        .ok_or(ReviewError::Conflict(
            "생성 시도의 provider snapshot이 유효하지 않습니다",
        ))?;
    let model = row
        .6
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| (1..=120).contains(&value.trim().chars().count()))
        .ok_or(ReviewError::Conflict(
            "생성 시도의 provider snapshot이 유효하지 않습니다",
        ))?;
    let creator: String =
        sqlx::query_scalar("SELECT name FROM content_provider_configs WHERE id=$1")
            .bind(provider_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ReviewError::Conflict(
                "생성 시도의 provider snapshot이 유효하지 않습니다",
            ))?;
    let source_revision = hex(&row.7);
    let derived = json!({"review_item_id":id,"revision":row.0,"artifact_hash":hex(&row.1),"origin_type":"ai_generated","creator_or_provider":creator,"source_url":null,"source_revision":source_revision,"license_basis":"provider_contract","license_identifier":input.license_identifier.trim(),"attribution":input.attribution.trim(),"modification_status":input.modification_status,"commercial_use_allowed":input.commercial_use_allowed,"redistribution_allowed":input.redistribution_allowed,"ai_provider_id":provider_id,"ai_provider_name":creator,"ai_model":model,"generation_job_id":row.4,"generation_attempt_id":row.5,"generation_run_reference":row.5.to_string(),"evidence_reference":input.evidence_reference.trim(),"evidence_hash":input.evidence_hash,"attachment_metadata":input.attachment_metadata,"legal_status":"pending"});
    let provenance_hash = receipt_hash(&derived)?;
    if let Some((existing,old_hash))=sqlx::query_as::<_,(Uuid,Vec<u8>)>("SELECT id,provenance_hash FROM content_provenance_records WHERE recorded_by=$1 AND review_item_id=$2 AND idempotency_key=$3").bind(actor).bind(id).bind(input.idempotency_key).fetch_optional(&mut *tx).await?{if old_hash!=provenance_hash{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 출처 기록을 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"id":existing,"provenance_hash":hex(&old_hash),"replayed":true})));}
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM content_provenance_records WHERE review_item_id=$1 AND revision_number=$2)").bind(id).bind(row.0).fetch_one(&mut *tx).await?{return Err(ReviewError::Conflict("현재 리비전의 출처 기록이 이미 존재합니다"));}
    let provenance_id:Uuid=sqlx::query_scalar("INSERT INTO content_provenance_records (review_item_id,revision_number,artifact_hash,origin_type,creator_or_provider,source_url,source_revision,license_basis,license_identifier,attribution,modification_status,commercial_use_allowed,redistribution_allowed,ai_provider_id,ai_provider_name,ai_model,generation_job_id,generation_attempt_id,generation_run_reference,evidence_reference,evidence_hash,attachment_metadata,legal_status,provenance_hash,recorded_by,idempotency_key) VALUES ($1,$2,$3,'ai_generated',$4,NULL,$5,'provider_contract',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,'pending',$20,$21,$22) RETURNING id").bind(id).bind(row.0).bind(&row.1).bind(&creator).bind(&source_revision).bind(input.license_identifier.trim()).bind(input.attribution.trim()).bind(&input.modification_status).bind(input.commercial_use_allowed).bind(input.redistribution_allowed).bind(provider_id).bind(&creator).bind(model).bind(row.4).bind(row.5).bind(row.5.to_string()).bind(input.evidence_reference.trim()).bind(evidence_hash).bind(&input.attachment_metadata).bind(&provenance_hash).bind(actor).bind(input.idempotency_key).fetch_one(&mut *tx).await?;
    sqlx::query(audit_query()).bind(actor).bind("content.provenance.recorded").bind(id.to_string()).bind(json!({"revision":row.0,"provenance_id":provenance_id,"provenance_hash":hex(&provenance_hash)})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":provenance_id,"provenance_hash":hex(&provenance_hash)}),
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RightsInput {
    provenance_id: Uuid,
    provenance_hash: String,
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
        || !(3..=4000).contains(&input.evidence.trim().chars().count())
        || !(3..=120).contains(&input.provider_terms_version.trim().chars().count())
    {
        return Err(ReviewError::Invalid("권리 검토 근거를 확인해 주세요"));
    }
    let provenance_hash = hex32(&input.provenance_hash)?;
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
    let provenance=sqlx::query_as::<_,(Vec<u8>,String,bool,bool,String)>("SELECT provenance_hash,legal_status,commercial_use_allowed,redistribution_allowed,license_basis FROM content_provenance_records WHERE id=$1 AND review_item_id=$2 AND revision_number=$3").bind(input.provenance_id).bind(id).bind(revision).fetch_optional(&mut *tx).await?.ok_or(ReviewError::Conflict("현재 리비전의 완전한 출처 기록이 필요합니다"))?;
    if provenance.0 != provenance_hash {
        return Err(ReviewError::Conflict(
            "출처 기록 해시가 현재 증거와 일치하지 않습니다",
        ));
    }
    let expected_basis = match provenance.4.as_str() {
        "copyright_owner" => "original",
        "provider_contract" => "contract",
        "license" => "licensed",
        "public_domain" => "public_domain",
        _ => return Err(ReviewError::Conflict("허용되지 않은 권리 근거입니다")),
    };
    let approved = input.decision == "approve"
        && provenance.1 == "pending"
        && provenance.2
        && provenance.3
        && input.basis == expected_basis
        && input.commercial_use_allowed == provenance.2
        && input.redistribution_allowed == provenance.3;
    if input.decision == "approve" && !approved {
        return Err(ReviewError::Conflict(
            "승인된 상업 이용·재배포 출처 증거와 제공자 약관이 필요합니다",
        ));
    }
    let next = if approved && impact == "standard" {
        "approved"
    } else if approved {
        "pilot_pending"
    } else {
        "changes_requested"
    };
    sqlx::query("INSERT INTO content_rights_review_receipts (review_item_id,revision_number,reviewer_user_id,provenance_id,provenance_hash,decision,basis,evidence,commercial_use_allowed,redistribution_allowed,provider_terms_version,idempotency_key,receipt_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(id).bind(revision).bind(actor).bind(input.provenance_id).bind(provenance_hash).bind(&input.decision).bind(&input.basis).bind(input.evidence.trim()).bind(input.commercial_use_allowed).bind(input.redistribution_allowed).bind(input.provider_terms_version.trim()).bind(input.idempotency_key).bind(canonical_hash).execute(&mut *tx).await?;
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
    source_reference: String,
    #[serde(with = "time::serde::rfc3339")]
    started_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    ended_at: time::OffsetDateTime,
    participants: i32,
    completion_rate: f64,
    failure_rate: f64,
    report_count: i32,
    rollback_ready: bool,
    rollback_evidence: String,
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
    if !(3..=200).contains(&input.cohort.trim().chars().count())
        || !(3..=1000).contains(&input.source_reference.trim().chars().count())
        || input.ended_at <= input.started_at
        || input.participants < 5
        || !(0.0..=1.0).contains(&input.completion_rate)
        || !(0.0..=1.0).contains(&input.failure_rate)
        || input.report_count < 0
        || !matches!(input.decision.as_str(), "pass" | "fail")
        || !(3..=2000).contains(&input.rollback_evidence.trim().chars().count())
        || !(3..=2000).contains(&input.note.trim().chars().count())
    {
        return Err(ReviewError::Invalid("파일럿 증거를 확인해 주세요"));
    }
    let canonical_hash = receipt_hash(&input)?;
    let evidence_hash = receipt_hash(
        &json!({"cohort":input.cohort,"source_reference":input.source_reference,"started_at":input.started_at,"ended_at":input.ended_at,"participants":input.participants,"completion_rate":input.completion_rate,"failure_rate":input.failure_rate,"report_count":input.report_count,"rollback_ready":input.rollback_ready,"rollback_evidence":input.rollback_evidence,"decision":input.decision,"note":input.note}),
    )?;
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
    sqlx::query("INSERT INTO content_pilot_receipts (review_item_id,revision_number,reviewer_user_id,cohort,source_reference,started_at,ended_at,participants,completion_rate,failure_rate,report_count,rollback_ready,rollback_evidence,decision,note,idempotency_key,receipt_hash,evidence_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)").bind(id).bind(revision).bind(actor).bind(input.cohort.trim()).bind(input.source_reference.trim()).bind(input.started_at).bind(input.ended_at).bind(input.participants).bind(input.completion_rate).bind(input.failure_rate).bind(input.report_count).bind(input.rollback_ready).bind(input.rollback_evidence.trim()).bind(&input.decision).bind(input.note.trim()).bind(input.idempotency_key).bind(canonical_hash).bind(evidence_hash).execute(&mut *tx).await?;
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

#[derive(Deserialize, Serialize)]
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
    if !(3..=2000).contains(&input.reason.trim().chars().count()) {
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
    if !(3..=2000).contains(&input.reason.trim().chars().count()) {
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
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalCompleteInput {
    reason: String,
    evidence: String,
    idempotency_key: Uuid,
}
pub async fn complete_removal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RemovalCompleteInput>,
) -> Result<Json<Value>, ReviewError> {
    let actor = user(&state, &headers, true).await?;
    require(&state, actor, "content.retire").await?;
    if !(3..=2000).contains(&input.reason.trim().chars().count())
        || !(3..=4000).contains(&input.evidence.trim().chars().count())
    {
        return Err(ReviewError::Invalid(
            "제거 완료 사유와 증거를 확인해 주세요",
        ));
    }
    let payload_hash = receipt_hash(&input)?;
    let mut tx = state.pool().begin().await?;
    let current: String =
        sqlx::query_scalar("SELECT state FROM content_review_items WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ReviewError::NotFound)?;
    if let Some(old_hash)=sqlx::query_scalar::<_,String>("SELECT metadata->>'payload_hash' FROM audit_events WHERE actor_user_id=$1 AND action='content.removal_completed' AND target_type='content_review' AND target_id=$2 AND metadata->>'idempotency_key'=$3").bind(actor).bind(id.to_string()).bind(input.idempotency_key.to_string()).fetch_optional(&mut *tx).await?{if old_hash!=hex(&payload_hash){return Err(ReviewError::Conflict("같은 멱등성 키에 다른 제거 증거를 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"state":current,"replayed":true})));}
    if current != "removal_pending" {
        return Err(ReviewError::Conflict(
            "제거 대기 상태에서만 완료할 수 있습니다",
        ));
    }
    sqlx::query(
        "UPDATE content_review_items SET state='removal_completed',updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(audit_query()).bind(actor).bind("content.removal_completed").bind(id.to_string()).bind(json!({"idempotency_key":input.idempotency_key,"reason":input.reason.trim(),"evidence":input.evidence.trim(),"payload_hash":hex(&payload_hash)})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"state":"removal_completed"})))
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
    let publication_evidence = if next == "published" {
        Some(sqlx::query_scalar::<_,Value>("SELECT jsonb_build_object('revision',item.revision_number,'artifact_hash',encode(item.artifact_hash,'hex'),'provenance_id',provenance.id,'provenance_hash',encode(provenance.provenance_hash,'hex'),'ai_receipt_ids',(SELECT jsonb_agg(ai.id ORDER BY ai.review_kind) FROM content_ai_review_receipts ai WHERE ai.review_item_id=item.id AND ai.revision_number=item.revision_number AND ai.outcome='pass'),'human_receipt_id',(SELECT human_decision.id FROM content_human_review_decisions human_decision WHERE human_decision.review_item_id=item.id AND human_decision.revision_number=item.revision_number AND human_decision.decision='approve' ORDER BY created_at DESC LIMIT 1),'rights_receipt_id',(SELECT rights.id FROM content_rights_review_receipts rights WHERE rights.review_item_id=item.id AND rights.revision_number=item.revision_number AND rights.decision='approve' ORDER BY created_at DESC LIMIT 1),'pilot_receipt_id',(SELECT pilot.id FROM content_pilot_receipts pilot WHERE pilot.review_item_id=item.id AND pilot.revision_number=item.revision_number AND pilot.decision='pass' ORDER BY created_at DESC LIMIT 1)) FROM content_review_items item JOIN content_provenance_records provenance ON provenance.review_item_id=item.id AND provenance.revision_number=item.revision_number AND provenance.artifact_hash=item.artifact_hash WHERE item.id=$1 AND provenance.legal_status='pending' AND provenance.commercial_use_allowed AND provenance.redistribution_allowed AND (SELECT count(*) FROM content_ai_review_receipts ai WHERE ai.review_item_id=item.id AND ai.revision_number=item.revision_number AND ai.outcome='pass')=3 AND EXISTS(SELECT 1 FROM content_human_review_decisions human_decision WHERE human_decision.review_item_id=item.id AND human_decision.revision_number=item.revision_number AND human_decision.decision='approve') AND EXISTS(SELECT 1 FROM content_rights_review_receipts rights WHERE rights.review_item_id=item.id AND rights.revision_number=item.revision_number AND rights.decision='approve' AND rights.provenance_id=provenance.id AND rights.provenance_hash=provenance.provenance_hash) AND (item.impact='standard' OR EXISTS(SELECT 1 FROM content_pilot_receipts pilot WHERE pilot.review_item_id=item.id AND pilot.revision_number=item.revision_number AND pilot.decision='pass'))").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::Conflict("정확한 리비전의 출처·AI·사람·권리·파일럿 증거가 필요합니다"))?)
    } else {
        None
    };
    sqlx::query("UPDATE content_review_items SET state=$2,published_by=CASE WHEN $2='published' THEN $3 ELSE published_by END,published_at=CASE WHEN $2='published' THEN now() ELSE published_at END,updated_at=now() WHERE id=$1").bind(id).bind(next).bind(actor).execute(&mut *tx).await?;
    sqlx::query(audit_query())
        .bind(actor)
        .bind(action)
        .bind(id.to_string())
        .bind(json!({"idempotency_key":key,"reason":reason,"publication_evidence":publication_evidence}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":next})))
}

#[derive(Deserialize, Serialize)]
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
    let request_hash = receipt_hash(&input)?;
    let mut tx = state.pool().begin().await?;
    let (author,revision,old_hash,state_name):(Uuid,i32,Vec<u8>,String)=sqlx::query_as("SELECT author_user_id,revision_number,artifact_hash,state FROM content_review_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if actor != author {
        return Err(ReviewError::Forbidden);
    }
    if let Some((existing,old_request_hash))=sqlx::query_as::<_,(i32,Vec<u8>)>("SELECT revision_number,request_hash FROM content_review_revisions WHERE review_item_id=$1 AND changed_by=$2 AND idempotency_key=$3").bind(id).bind(actor).bind(input.idempotency_key).fetch_optional(&mut *tx).await?{if old_request_hash!=request_hash{return Err(ReviewError::Conflict("같은 멱등성 키에 다른 리비전을 사용할 수 없습니다"));}tx.commit().await?;return Ok(Json(json!({"state":state_name,"revision":existing,"replayed":true})));}
    if !matches!(
        state_name.as_str(),
        "changes_requested" | "unpublished" | "removal_completed"
    ) {
        return Err(ReviewError::Conflict(
            "변경 요청·게시 해제·제거 상태에서만 새 리비전을 만들 수 있습니다",
        ));
    }
    let hash:Vec<u8>=sqlx::query_scalar("SELECT artifact.content_hash FROM content_artifacts artifact JOIN content_generation_jobs job ON job.id=artifact.job_id WHERE artifact.id=$1 AND artifact.artifact_kind='candidate' AND job.created_by=$2").bind(input.artifact_id).bind(actor).fetch_optional(&mut *tx).await?.ok_or(ReviewError::NotFound)?;
    if hash == old_hash {
        return Err(ReviewError::Conflict(
            "새 리비전은 다른 아티팩트 해시가 필요합니다",
        ));
    }
    let next = revision + 1;
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key,request_hash) VALUES ($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(next).bind(input.artifact_id).bind(&hash).bind(actor).bind(input.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
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
    let receipts:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'kind',review_kind,'provider',provider,'model',model,'prompt_version',prompt_version,'role_identifier',role_identifier,'prompt_hash',encode(prompt_hash,'hex'),'review_seed',review_seed,'review_context',review_context,'context_hash',encode(context_hash,'hex'),'review_response',review_response,'input_hash',encode(input_hash,'hex'),'output_hash',encode(output_hash,'hex'),'latency_ms',latency_ms,'usage',usage,'outcome',outcome,'findings',findings,'created_at',created_at) FROM content_ai_review_receipts WHERE review_item_id=$1 AND revision_number=$2 ORDER BY review_kind").bind(id).bind(row.2).fetch_all(state.pool()).await?;
    let provenance:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'origin_type',origin_type,'creator_or_provider',creator_or_provider,'source_url',source_url,'source_revision',source_revision,'license_basis',license_basis,'license_identifier',license_identifier,'attribution',attribution,'modification_status',modification_status,'commercial_use_allowed',commercial_use_allowed,'redistribution_allowed',redistribution_allowed,'ai_provider_id',ai_provider_id,'ai_provider_name',ai_provider_name,'ai_model',ai_model,'generation_job_id',generation_job_id,'generation_attempt_id',generation_attempt_id,'generation_run_reference',generation_run_reference,'evidence_reference',evidence_reference,'evidence_hash',encode(evidence_hash,'hex'),'attachment_metadata',attachment_metadata,'legal_status',legal_status,'provenance_hash',encode(provenance_hash,'hex'),'created_at',created_at) FROM content_provenance_records WHERE review_item_id=$1 AND revision_number=$2").bind(id).bind(row.2).fetch_optional(state.pool()).await?;
    let human_receipts:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'reviewer_user_id',reviewer_user_id,'decision',decision,'note',note,'created_at',created_at) FROM content_human_review_decisions WHERE review_item_id=$1 AND revision_number=$2 ORDER BY created_at").bind(id).bind(row.2).fetch_all(state.pool()).await?;
    let rights_receipts:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'reviewer_user_id',reviewer_user_id,'provenance_id',provenance_id,'provenance_hash',encode(provenance_hash,'hex'),'decision',decision,'basis',basis,'evidence',evidence,'commercial_use_allowed',commercial_use_allowed,'redistribution_allowed',redistribution_allowed,'provider_terms_version',provider_terms_version,'created_at',created_at) FROM content_rights_review_receipts WHERE review_item_id=$1 AND revision_number=$2 ORDER BY created_at").bind(id).bind(row.2).fetch_all(state.pool()).await?;
    let pilot_receipts:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'reviewer_user_id',reviewer_user_id,'cohort',cohort,'source_reference',source_reference,'started_at',started_at,'ended_at',ended_at,'participants',participants,'completion_rate',completion_rate,'failure_rate',failure_rate,'report_count',report_count,'rollback_ready',rollback_ready,'rollback_evidence',rollback_evidence,'decision',decision,'note',note,'evidence_hash',encode(evidence_hash,'hex'),'created_at',created_at) FROM content_pilot_receipts WHERE review_item_id=$1 AND revision_number=$2 ORDER BY created_at").bind(id).bind(row.2).fetch_all(state.pool()).await?;
    let audit:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('action',action,'actor_user_id',actor_user_id,'metadata',metadata,'occurred_at',occurred_at) FROM audit_events WHERE target_type='content_review' AND target_id=$1 ORDER BY occurred_at").bind(id.to_string()).fetch_all(state.pool()).await?;
    Ok(Json(
        json!({"id":id,"artifact_id":row.1,"revision":row.2,"state":row.3,"artifact_hash":row.4,"artifact":row.5,"provenance":provenance,"ai_receipts":receipts,"human_receipts":human_receipts,"rights_receipts":rights_receipts,"pilot_receipts":pilot_receipts,"audit":audit}),
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
