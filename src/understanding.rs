#![allow(clippy::type_complexity)]

pub struct UnderstandingEvidence {
    pub artifact_hash: [u8; 32],
    pub explanation: String,
    pub expected_concepts: Vec<String>,
    pub modification_run_hash: [u8; 32],
    pub transfer_run_hash: [u8; 32],
    pub hidden_checks_passed: bool,
    pub ai_assessed: bool,
}

pub fn evaluate_understanding(
    evidence: &UnderstandingEvidence,
) -> Result<&'static str, &'static str> {
    if evidence.ai_assessed {
        return Err("AI 평가는 최종 이해 증거가 될 수 없습니다");
    }
    if !evidence.hidden_checks_passed
        || evidence.modification_run_hash == evidence.artifact_hash
        || evidence.transfer_run_hash == evidence.artifact_hash
        || evidence.transfer_run_hash == evidence.modification_run_hash
        || evidence.expected_concepts.is_empty()
        || evidence
            .expected_concepts
            .iter()
            .any(|concept| !evidence.explanation.contains(concept))
    {
        return Err("설명·독립 변형·다른 맥락 전이 증거가 모두 필요합니다");
    }
    Ok("transfer_verified")
}

#[derive(Debug)]
pub enum UnderstandingError {
    Auth(AuthError),
    Invalid(&'static str),
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}
impl From<AuthError> for UnderstandingError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}
impl From<sqlx::Error> for UnderstandingError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl IntoResponse for UnderstandingError {
    fn into_response(self) -> Response {
        let (s, c, m) = match self {
            Self::Auth(e) => return e.into_response(),
            Self::Invalid(m) => (StatusCode::BAD_REQUEST, "invalid_understanding", m),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "understanding_not_found",
                "이해 검증 대상을 찾을 수 없습니다",
            ),
            Self::Conflict(m) => (StatusCode::CONFLICT, "understanding_conflict", m),
            Self::Database(e) => {
                tracing::error!(%e,"이해 검증 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
        };
        (s, Json(json!({"error":{"code":c,"message":m}}))).into_response()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateChallengeRequest {
    workspace_id: Uuid,
    kind: String,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitChallengeRequest {
    explanation: String,
    modification: String,
    modification_run_id: Uuid,
    transfer_run_id: Uuid,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitPredictionsRequest {
    modification_prediction: String,
    transfer_prediction: String,
    idempotency_key: Uuid,
}

fn signed_receipt_valid(
    state: &AppState,
    run_id: Uuid,
    hash: Option<&Vec<u8>>,
    mac: Option<&Vec<u8>>,
) -> bool {
    hash.zip(mac).is_some_and(|(hash, mac)| {
        crate::workspaces::verify_receipt(
            state.settings().workspace_receipt_secret.as_bytes(),
            run_id,
            hash,
            mac,
        )
    })
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateChallengeRequest>,
) -> Result<(StatusCode, Json<Value>), UnderstandingError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.kind != "explanation_modification_transfer" {
        return Err(UnderstandingError::Invalid(
            "지원되는 이해 검증 유형이 아닙니다",
        ));
    }
    let mut tx = state.pool().begin().await?;
    let request_hash = Sha256::digest(
        serde_json::to_vec(&json!({"workspace_id":req.workspace_id,"kind":req.kind}))
            .map_err(|_| UnderstandingError::Invalid("과제 요청을 처리할 수 없습니다"))?,
    )
    .to_vec();
    sqlx::query("SELECT id FROM project_workspaces WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(req.workspace_id)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(UnderstandingError::NotFound)?;
    if let Some((id,state,prompt,old,committed))=sqlx::query_as::<_,(Uuid,String,SqlJson<Value>,Vec<u8>,bool)>("SELECT id,state,prompt,request_hash,predictions_committed_at IS NOT NULL FROM understanding_challenges WHERE user_id=$1 AND idempotency_key=$2").bind(user).bind(req.idempotency_key).fetch_optional(&mut *tx).await?{if old!=request_hash{return Err(UnderstandingError::Conflict("같은 멱등키에 다른 이해 과제를 사용할 수 없습니다"));}tx.commit().await?;return Ok((StatusCode::OK,Json(json!({"id":id,"state":state,"prompt":prompt.0,"predictions_committed":committed}))));}
    let source:Option<(Uuid,Vec<u8>,Vec<u8>,Vec<u8>,String,String,Vec<u8>,Vec<u8>,String,SqlJson<Value>,Option<Vec<u8>>,Option<Vec<u8>>)>=sqlx::query_as("SELECT r.id,r.artifact_hash,r.semantic_hash,r.stdout_hash,r.stdout,r.check_suite_id,r.check_suite_hash,r.template_digest,t.track_kind,r.artifact,r.runner_receipt_hash,r.runner_receipt_mac FROM workspace_runs r JOIN workspace_templates t ON(t.id,t.revision,t.template_digest)=(r.template_id,r.template_revision,r.template_digest) WHERE r.workspace_id=$1 AND r.user_id=$2 AND r.status='succeeded' AND r.deterministic_checks_passed ORDER BY r.completed_at DESC LIMIT 1").bind(req.workspace_id).bind(user).fetch_optional(&mut *tx).await?;
    let (
        run,
        hash,
        semantic,
        stdout_hash,
        stdout,
        suite,
        suite_hash,
        template_digest,
        track,
        artifact,
        receipt_hash,
        receipt_mac,
    ) = source.ok_or(UnderstandingError::Conflict(
        "작업자가 검증한 성공 실행이 먼저 필요합니다",
    ))?;
    if !signed_receipt_valid(&state, run, receipt_hash.as_ref(), receipt_mac.as_ref()) {
        return Err(UnderstandingError::Conflict(
            "서명된 작업자 실행 영수증이 필요합니다",
        ));
    }
    if let Some((existing,state_name,prompt,committed))=sqlx::query_as::<_,(Uuid,String,SqlJson<Value>,bool)>("SELECT id,state,prompt,predictions_committed_at IS NOT NULL FROM understanding_challenges WHERE user_id=$1 AND workspace_id=$2 AND source_run_id=$3 AND state='pending'").bind(user).bind(req.workspace_id).bind(run).fetch_optional(&mut *tx).await? {
        tx.commit().await?;
        return Ok((StatusCode::OK,Json(json!({"id":existing,"state":state_name,"prompt":prompt.0,"predictions_committed":committed}))));
    }
    let paths = artifact
        .0
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["path"].as_str())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err(UnderstandingError::Conflict(
            "검증할 소스 파일 사실이 없습니다",
        ));
    }
    let id = Uuid::now_v7();
    let prompt = json!({"explanation":format!("{} 파일의 상태 전이와 오류 처리를 설명하세요",paths.join(", ")),"prediction":"독립 변형 실행의 정확한 출력을 예측하세요","modification":"원 작업공간에서 의미 있는 코드 변경 후 modification 실행을 만드세요","transfer":"다른 템플릿 맥락에서 transfer 실행을 만드세요"});
    let concepts = json!(paths);
    let facts = json!({"paths":concepts.clone(),"source_stdout":stdout,"check_suite_id":suite});
    sqlx::query("INSERT INTO understanding_challenges(id,user_id,workspace_id,source_run_id,artifact_hash,source_semantic_hash,source_stdout_hash,source_stdout,check_suite_id,check_suite_hash,source_template_digest,source_track_kind,source_facts,kind,prompt,expected_concepts,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)").bind(id).bind(user).bind(req.workspace_id).bind(run).bind(hash).bind(semantic).bind(stdout_hash).bind(&stdout).bind(suite).bind(suite_hash).bind(template_digest).bind(track).bind(SqlJson(facts)).bind(req.kind).bind(SqlJson(prompt.clone())).bind(SqlJson(concepts)).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"state":"pending","prompt":prompt,"predictions_committed":false})),
    ))
}

pub async fn commit_predictions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<CommitPredictionsRequest>,
) -> Result<(StatusCode, Json<Value>), UnderstandingError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.modification_prediction.trim().is_empty() || req.transfer_prediction.trim().is_empty() {
        return Err(UnderstandingError::Invalid(
            "변형·전이 결과 예측을 모두 작성해 주세요",
        ));
    }
    let request_hash=Sha256::digest(serde_json::to_vec(&json!({"challenge_id":id,"modification_prediction":req.modification_prediction.trim(),"transfer_prediction":req.transfer_prediction.trim()})).map_err(|_|UnderstandingError::Invalid("예측을 처리할 수 없습니다"))?).to_vec();
    let mut tx = state.pool().begin().await?;
    let row:Option<(Option<Uuid>,Option<Vec<u8>>,String)>=sqlx::query_as("SELECT prediction_idempotency_key,prediction_request_hash,state FROM understanding_challenges WHERE id=$1 AND user_id=$2 FOR UPDATE").bind(id).bind(user).fetch_optional(&mut *tx).await?;
    let (old_key, old_hash, state_name) = row.ok_or(UnderstandingError::NotFound)?;
    if let Some(old_key) = old_key {
        if old_key != req.idempotency_key || old_hash.as_ref() != Some(&request_hash) {
            return Err(UnderstandingError::Conflict(
                "이미 다른 예측을 확정했습니다",
            ));
        }
        tx.commit().await?;
        return Ok((
            StatusCode::OK,
            Json(json!({"id":id,"predictions_committed":true})),
        ));
    }
    if state_name != "pending"
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspace_runs WHERE challenge_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(UnderstandingError::Conflict(
            "검증 실행 전에만 예측을 확정할 수 있습니다",
        ));
    }
    sqlx::query("UPDATE understanding_challenges SET modification_prediction=$3,transfer_prediction=$4,predictions_committed_at=now(),prediction_idempotency_key=$5,prediction_request_hash=$6 WHERE id=$1 AND user_id=$2").bind(id).bind(user).bind(req.modification_prediction.trim()).bind(req.transfer_prediction.trim()).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"predictions_committed":true})),
    ))
}
pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitChallengeRequest>,
) -> Result<Json<Value>, UnderstandingError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.explanation.trim().len() < 20 || req.modification.trim().len() < 5 {
        return Err(UnderstandingError::Invalid(
            "설명·예측·변형·전이 답변을 모두 작성해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    let request_hash=Sha256::digest(serde_json::to_vec(&json!({"challenge_id":id,"explanation":req.explanation,"modification":req.modification,"modification_run_id":req.modification_run_id,"transfer_run_id":req.transfer_run_id})).map_err(|_|UnderstandingError::Invalid("이해 응답을 처리할 수 없습니다"))?).to_vec();
    let row:Option<(Uuid,Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>,SqlJson<Vec<String>>,String,Option<String>,Option<String>,SqlJson<Value>)>=sqlx::query_as("SELECT c.workspace_id,c.artifact_hash,c.source_semantic_hash,c.source_stdout_hash,c.check_suite_hash,c.source_template_digest,c.expected_concepts,c.state,c.modification_prediction,c.transfer_prediction,r.artifact FROM understanding_challenges c JOIN workspace_runs r ON r.id=c.source_run_id WHERE c.id=$1 AND c.user_id=$2 FOR UPDATE OF c").bind(id).bind(user).fetch_optional(&mut *tx).await?;
    let (
        row_ws,
        source,
        source_semantic,
        source_stdout_hash,
        source_suite,
        source_template,
        source_paths,
        challenge_state,
        modification_prediction,
        transfer_prediction,
        source_artifact,
    ) = row.ok_or(UnderstandingError::NotFound)?;
    let modification_prediction = modification_prediction.ok_or(UnderstandingError::Conflict(
        "실행 전 예측 확정이 필요합니다",
    ))?;
    let transfer_prediction = transfer_prediction.ok_or(UnderstandingError::Conflict(
        "실행 전 예측 확정이 필요합니다",
    ))?;
    if let Some((existing,old_challenge,old_hash)) = sqlx::query_as::<_, (String,Uuid,Vec<u8>)>(
        "SELECT state,challenge_id,request_hash FROM understanding_receipts WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(user)
    .bind(req.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if old_challenge!=id||old_hash!=request_hash{return Err(UnderstandingError::Conflict("같은 멱등키에 다른 이해 응답을 사용할 수 없습니다"));}
        tx.commit().await?;
        return Ok(Json(
            json!({"state":existing,"evidence_labels":["EXPLAINED","INDEPENDENTLY_MODIFIED","TRANSFER_VERIFIED"],"assistance_disclosure":"결정론적 숨은 검사로 검증됨"}),
        ));
    }
    if challenge_state != "pending" {
        return Err(UnderstandingError::Conflict("이미 완료된 이해 검증입니다"));
    }
    let runs:Vec<(Uuid,Uuid,Vec<u8>,Vec<u8>,Vec<u8>,String,String,Option<Uuid>,bool,Option<Vec<u8>>,Vec<u8>,Vec<u8>,Option<Vec<u8>>,SqlJson<Value>)>=sqlx::query_as("SELECT id,workspace_id,artifact_hash,semantic_hash,stdout_hash,stdout,validation_kind,challenge_id,deterministic_checks_passed,runner_receipt_hash,check_suite_hash,template_digest,runner_receipt_mac,artifact FROM workspace_runs WHERE user_id=$1 AND id=ANY($2) AND status='succeeded'").bind(user).bind(vec![req.modification_run_id,req.transfer_run_id]).fetch_all(&mut *tx).await?;
    if runs.len() != 2 {
        return Err(UnderstandingError::Invalid(
            "작업자가 검증한 변형·전이 실행이 필요합니다",
        ));
    }
    let modification = runs
        .iter()
        .find(|r| r.0 == req.modification_run_id)
        .unwrap();
    let transfer = runs.iter().find(|r| r.0 == req.transfer_run_id).unwrap();
    let exact = modification.1 == row_ws
        && transfer.1 != row_ws
        && modification.6 == "modification"
        && transfer.6 == "transfer"
        && modification.7 == Some(id)
        && transfer.7 == Some(id)
        && modification.8
        && transfer.8
        && modification.9.is_some()
        && transfer.9.is_some()
        && signed_receipt_valid(
            &state,
            modification.0,
            modification.9.as_ref(),
            modification.12.as_ref(),
        )
        && signed_receipt_valid(
            &state,
            transfer.0,
            transfer.9.as_ref(),
            transfer.12.as_ref(),
        )
        && modification.10 == source_suite
        && transfer.10 != source_suite
        && modification.11 == source_template
        && transfer.11 != source_template
        && modification.3 != source_semantic
        && modification.4 != source_stdout_hash
        && transfer.3 != source_semantic
        && transfer.4 != source_stdout_hash
        && modification.3 != transfer.3;
    if !exact {
        return Err(UnderstandingError::Invalid(
            "과제에 결합된 의미 변경과 다른 템플릿 전이 검증 영수증이 필요합니다",
        ));
    }
    if modification_prediction.trim() != modification.5.trim() {
        return Err(UnderstandingError::Invalid(
            "변경 결과 예측이 검증 실행 출력과 다릅니다",
        ));
    }
    if transfer_prediction.trim() != transfer.5.trim() {
        return Err(UnderstandingError::Invalid(
            "전이 결과 답변이 검증 실행 출력과 다릅니다",
        ));
    }
    if source_paths
        .0
        .iter()
        .any(|path| !req.explanation.contains(path))
    {
        return Err(UnderstandingError::Invalid(
            "설명에 서버가 확인한 소스 파일 흐름이 빠졌습니다",
        ));
    }
    let source_files = source_artifact.0.as_array().cloned().unwrap_or_default();
    let changed_paths = modification
        .13
        .0
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let path = file["path"].as_str()?;
            let content = file["content"].as_str()?;
            let changed = !source_files
                .iter()
                .any(|source| source["path"] == path && source["content"] == content);
            changed.then_some(path)
        })
        .collect::<Vec<_>>();
    if changed_paths.is_empty()
        || !changed_paths
            .iter()
            .any(|path| req.modification.contains(path))
    {
        return Err(UnderstandingError::Invalid(
            "독립 변형 설명에 서버가 확인한 변경 파일을 포함해야 합니다",
        ));
    }
    let mh = modification.2.clone();
    let th = transfer.2.clone();
    let result = "transfer_verified";
    let receipt_value = json!({"challenge":id,"source":source,"explanation":req.explanation,"modification_prediction":modification_prediction,"modification":req.modification,"transfer_prediction":transfer_prediction,"modification_run":req.modification_run_id,"transfer_run":req.transfer_run_id,"state":result});
    let receipt_hash = Sha256::digest(
        serde_json::to_vec(&receipt_value)
            .map_err(|_| UnderstandingError::Invalid("영수증을 처리할 수 없습니다"))?,
    )
    .to_vec();
    sqlx::query("INSERT INTO understanding_receipts(id,challenge_id,user_id,source_artifact_hash,explanation,modification_run_id,modification_hash,transfer_run_id,transfer_hash,deterministic_checks_passed,ai_assessed,state,receipt_hash,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,true,false,$10,$11,$12,$13)").bind(Uuid::now_v7()).bind(id).bind(user).bind(source).bind(req.explanation).bind(req.modification_run_id).bind(mh).bind(req.transfer_run_id).bind(th).bind(result).bind(receipt_hash).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE understanding_challenges SET state='transfer_verified' WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"state":result,"evidence_labels":["EXPLAINED","INDEPENDENTLY_MODIFIED","TRANSFER_VERIFIED"],"assistance_disclosure":"결정론적 숨은 검사로 검증됨"}),
    ))
}
use crate::{auth::AuthError, http::AppState};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::types::Json as SqlJson;
use uuid::Uuid;
