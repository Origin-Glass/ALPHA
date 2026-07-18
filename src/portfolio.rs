#![allow(clippy::type_complexity)]

use crate::{
    auth::{self, AuthError},
    http::AppState,
};
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

pub const EVIDENCE_LABELS: [&str; 7] = [
    "BUILT",
    "TESTED",
    "DEBUGGED",
    "EXPLAINED",
    "INDEPENDENTLY_MODIFIED",
    "TRANSFER_VERIFIED",
    "MAINTAINED",
];
pub fn valid_evidence_labels(labels: &[String]) -> bool {
    !labels.is_empty()
        && labels
            .iter()
            .all(|label| EVIDENCE_LABELS.contains(&label.as_str()))
}

#[derive(Debug)]
pub enum PortfolioError {
    Auth(AuthError),
    Invalid(&'static str),
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}
impl From<AuthError> for PortfolioError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}
impl From<sqlx::Error> for PortfolioError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl IntoResponse for PortfolioError {
    fn into_response(self) -> Response {
        let (s, c, m) = match self {
            Self::Auth(e) => return e.into_response(),
            Self::Invalid(m) => (StatusCode::BAD_REQUEST, "invalid_portfolio", m),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "portfolio_not_found",
                "포트폴리오를 찾을 수 없습니다",
            ),
            Self::Conflict(m) => (StatusCode::CONFLICT, "portfolio_conflict", m),
            Self::Database(e) => {
                tracing::error!(%e,"포트폴리오 처리 실패");
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
pub struct CreatePortfolioRequest {
    title: String,
    problem: String,
    target_user: String,
    workspace_id: Uuid,
    source_run_id: Uuid,
    rights_receipt_id: Uuid,
    visibility: String,
    assistance_disclosure: String,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RightsRequest {
    source_run_id: Uuid,
    provenance: String,
    license_identifier: String,
    idempotency_key: Uuid,
}

async fn item(pool: &sqlx::PgPool, user: Uuid, id: Uuid) -> Result<Value, PortfolioError> {
    let row:Option<(Uuid,String,String,String,String,String,SqlJson<Value>,String)>=sqlx::query_as("SELECT id,title,problem,target_user,visibility,assistance_disclosure,evidence_labels,status FROM portfolio_items WHERE id=$1 AND user_id=$2").bind(id).bind(user).fetch_optional(pool).await?;
    row.map(|r|json!({"id":r.0,"title":r.1,"problem":r.2,"target_user":r.3,"visibility":r.4,"assistance_disclosure":r.5,"evidence_labels":r.6.0,"status":r.7})).ok_or(PortfolioError::NotFound)
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreatePortfolioRequest>,
) -> Result<(StatusCode, Json<Value>), PortfolioError> {
    let user = auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.title.trim().is_empty()
        || req.problem.trim().is_empty()
        || req.target_user.trim().is_empty()
        || req.assistance_disclosure.trim().len() < 2
        || !matches!(req.visibility.as_str(), "private" | "public")
    {
        return Err(PortfolioError::Invalid(
            "포트폴리오 내용·공개·증거 라벨을 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    let request_hash=Sha256::digest(serde_json::to_vec(&json!({"title":req.title.trim(),"problem":req.problem.trim(),"target_user":req.target_user.trim(),"workspace_id":req.workspace_id,"source_run_id":req.source_run_id,"rights_receipt_id":req.rights_receipt_id,"visibility":req.visibility,"assistance_disclosure":req.assistance_disclosure.trim()})).map_err(|_|PortfolioError::Invalid("포트폴리오 요청을 처리할 수 없습니다"))?).to_vec();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((id, old)) = sqlx::query_as::<_, (Uuid, Vec<u8>)>(
        "SELECT id,request_hash FROM portfolio_items WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(user)
    .bind(req.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if old != request_hash {
            return Err(PortfolioError::Conflict(
                "같은 멱등키에 다른 포트폴리오를 사용할 수 없습니다",
            ));
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(item(state.pool(), user, id).await?)));
    }
    let exact:Option<(bool,bool,bool,Option<Vec<u8>>,Option<Vec<u8>>)>=sqlx::query_as("SELECT r.status='succeeded' AND r.deterministic_checks_passed AND rights.workspace_version=r.workspace_version AND rights.artifact_hash=r.artifact_hash,EXISTS(SELECT 1 FROM understanding_receipts u WHERE u.user_id=r.user_id AND u.transfer_run_id=r.id AND u.state='transfer_verified' AND u.deterministic_checks_passed),r.supports_tests_snapshot,r.runner_receipt_hash,r.runner_receipt_mac FROM workspace_runs r JOIN portfolio_rights_receipts rights ON(rights.workspace_id,rights.user_id)=(r.workspace_id,r.user_id) WHERE r.id=$1 AND r.user_id=$2 AND r.workspace_id=$3 AND rights.id=$4 AND rights.decision='approved'").bind(req.source_run_id).bind(user).bind(req.workspace_id).bind(req.rights_receipt_id).fetch_optional(&mut *tx).await?;
    let (mut run_verified, understanding_verified, supports_tests, receipt_hash, receipt_mac) =
        exact.ok_or(PortfolioError::Invalid(
            "정확한 산출물 리비전의 권리 승인 영수증이 필요합니다",
        ))?;
    run_verified &= receipt_hash
        .as_ref()
        .zip(receipt_mac.as_ref())
        .is_some_and(|(hash, mac)| {
            crate::workspaces::verify_receipt(
                state.settings().workspace_receipt_secret.as_bytes(),
                req.source_run_id,
                hash,
                mac,
            )
        });
    let mut allowed = if run_verified { vec!["BUILT"] } else { vec![] };
    if supports_tests {
        allowed.push("TESTED")
    }
    if understanding_verified {
        allowed.extend(["EXPLAINED", "INDEPENDENTLY_MODIFIED", "TRANSFER_VERIFIED"])
    }
    if !run_verified {
        return Err(PortfolioError::Invalid(
            "서명된 성공 실행 증거가 필요합니다",
        ));
    }
    let disclosure = format!(
        "학습자 공개: {}; 자동 검증: {}",
        req.assistance_disclosure.trim(),
        allowed.join(",")
    );
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO portfolio_items(id,user_id,workspace_id,source_run_id,rights_receipt_id,title,problem,target_user,visibility,assistance_disclosure,evidence_labels,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(id).bind(user).bind(req.workspace_id).bind(req.source_run_id).bind(req.rights_receipt_id).bind(req.title.trim()).bind(req.problem.trim()).bind(req.target_user.trim()).bind(req.visibility).bind(disclosure).bind(SqlJson(json!(allowed))).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(item(state.pool(), user, id).await?),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, PortfolioError> {
    let user = auth::authenticated_user_id(&state, &headers).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM portfolio_items WHERE user_id=$1 ORDER BY created_at DESC",
    )
    .bind(user)
    .fetch_all(state.pool())
    .await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(item(state.pool(), user, id).await?)
    }
    Ok(Json(json!({"items":items})))
}
pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, PortfolioError> {
    let user = auth::authenticated_user_id(&state, &headers).await?;
    Ok(Json(item(state.pool(), user, id).await?))
}
pub async fn public_detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, PortfolioError> {
    let row:Option<(String,String,String,String,SqlJson<Value>)>=sqlx::query_as("SELECT title,problem,target_user,assistance_disclosure,evidence_labels FROM portfolio_items WHERE id=$1 AND visibility='public' AND status='published'").bind(id).fetch_optional(state.pool()).await?;
    let r = row.ok_or(PortfolioError::NotFound)?;
    Ok(Json(
        json!({"id":id,"title":r.0,"problem":r.1,"target_user":r.2,"assistance_disclosure":r.3,"evidence_labels":r.4.0,"status":"published"}),
    ))
}
pub async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, PortfolioError> {
    let user = auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let changed=sqlx::query("UPDATE portfolio_items p SET status='published',published_at=now() FROM workspace_runs r,portfolio_rights_receipts rights WHERE p.id=$1 AND p.user_id=$2 AND p.visibility='public' AND p.status='draft' AND r.id=p.source_run_id AND r.user_id=p.user_id AND r.status='succeeded' AND r.deterministic_checks_passed AND r.runner_receipt_hash IS NOT NULL AND rights.id=p.rights_receipt_id AND rights.user_id=p.user_id AND rights.decision='approved' AND rights.workspace_version=r.workspace_version AND rights.artifact_hash=r.artifact_hash").bind(id).bind(user).execute(state.pool()).await?;
    if changed.rows_affected() != 1 {
        return Err(PortfolioError::Conflict(
            "공개 설정과 정확한 권리 승인 증거가 필요합니다",
        ));
    }
    Ok(Json(item(state.pool(), user, id).await?))
}

pub async fn approve_rights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RightsRequest>,
) -> Result<(StatusCode, Json<Value>), PortfolioError> {
    let reviewer = auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !crate::governance::has_capability(state.pool(), reviewer, "rights.review").await? {
        return Err(PortfolioError::Auth(AuthError::Forbidden));
    }
    if !matches!(
        req.provenance.as_str(),
        "original" | "licensed" | "authorized_import"
    ) || !(2..=120).contains(&req.license_identifier.len())
    {
        return Err(PortfolioError::Invalid(
            "출처와 라이선스 근거를 확인해 주세요",
        ));
    }
    let canonical = json!({"source_run_id":req.source_run_id,"provenance":req.provenance,"license_identifier":req.license_identifier});
    let request_hash = Sha256::digest(
        serde_json::to_vec(&canonical)
            .map_err(|_| PortfolioError::Invalid("권리 입력을 처리할 수 없습니다"))?,
    )
    .to_vec();
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(reviewer)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((id,old,user))=sqlx::query_as::<_,(Uuid,Vec<u8>,Uuid)>("SELECT id,request_hash,user_id FROM portfolio_rights_receipts WHERE reviewed_by=$1 AND idempotency_key=$2").bind(reviewer).bind(req.idempotency_key).fetch_optional(&mut *tx).await?{if old!=request_hash{return Err(PortfolioError::Conflict("같은 멱등키에 다른 권리 검토를 사용할 수 없습니다"))}tx.commit().await?;return Ok((StatusCode::OK,Json(json!({"id":id,"user_id":user,"decision":"approved"}))))}
    let run:Option<(Uuid,Uuid,i32,Vec<u8>,Option<Vec<u8>>,Option<Vec<u8>>)>=sqlx::query_as("SELECT workspace_id,user_id,workspace_version,artifact_hash,runner_receipt_hash,runner_receipt_mac FROM workspace_runs WHERE id=$1 AND status='succeeded' AND deterministic_checks_passed FOR UPDATE").bind(req.source_run_id).fetch_optional(&mut *tx).await?;
    let (workspace, user, version, artifact, run_hash, run_mac) =
        run.ok_or(PortfolioError::NotFound)?;
    if !run_hash
        .as_ref()
        .zip(run_mac.as_ref())
        .is_some_and(|(hash, mac)| {
            crate::workspaces::verify_receipt(
                state.settings().workspace_receipt_secret.as_bytes(),
                req.source_run_id,
                hash,
                mac,
            )
        })
    {
        return Err(PortfolioError::Invalid(
            "서명된 작업자 실행 영수증이 필요합니다",
        ));
    }
    if user == reviewer {
        return Err(PortfolioError::Conflict(
            "본인 산출물의 권리를 직접 승인할 수 없습니다",
        ));
    }
    let id = Uuid::now_v7();
    let receipt_hash=Sha256::digest(serde_json::to_vec(&json!({"id":id,"reviewer":reviewer,"user":user,"workspace":workspace,"version":version,"artifact":artifact,"request":canonical})).unwrap_or_default()).to_vec();
    sqlx::query("INSERT INTO portfolio_rights_receipts(id,workspace_id,user_id,workspace_version,artifact_hash,provenance,license_identifier,decision,reviewed_by,receipt_hash,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,'approved',$8,$9,$10,$11)").bind(id).bind(workspace).bind(user).bind(version).bind(artifact).bind(req.provenance).bind(req.license_identifier).bind(reviewer).bind(receipt_hash).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"user_id":user,"decision":"approved"})),
    ))
}
