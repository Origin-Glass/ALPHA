#![allow(clippy::type_complexity)]

use std::{
    collections::HashSet,
    path::{Component, Path},
    time::Duration,
};

use crate::{auth::AuthError, http::AppState};
use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::types::Json as SqlJson;
use uuid::Uuid;

pub const MAX_FILES: usize = 64;
pub const MAX_FILE_BYTES: usize = 262_144;
pub const MAX_WORKSPACE_BYTES: usize = 2_097_152;
pub const MAX_PATH_BYTES: usize = 160;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFileInput {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub symlink: bool,
}

impl WorkspaceFileInput {
    pub fn new(path: impl Into<String>, content: impl Into<String>, symlink: bool) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            symlink,
        }
    }
}

pub fn validate_files(files: &[WorkspaceFileInput]) -> Result<(), &'static str> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err("파일 수 제한을 벗어났습니다");
    }
    let mut names = HashSet::with_capacity(files.len());
    let mut total = 0_usize;
    for file in files {
        let path = Path::new(&file.path);
        if file.symlink
            || file.path.is_empty()
            || file.path.len() > MAX_PATH_BYTES
            || file.path.contains('\0')
            || file.path.contains("//")
            || file.path.ends_with('/')
            || file.path.contains('\\')
            || !file.path.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            })
            || path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !names.insert(file.path.to_lowercase())
        {
            return Err("안전하지 않거나 중복된 파일 경로입니다");
        }
        if file.content.len() > MAX_FILE_BYTES {
            return Err("개별 파일 크기 제한을 초과했습니다");
        }
        total = total
            .checked_add(file.content.len())
            .ok_or("작업공간 크기가 너무 큽니다")?;
    }
    if total > MAX_WORKSPACE_BYTES {
        Err("작업공간 크기 제한을 초과했습니다")
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Auth(AuthError),
    Invalid(&'static str),
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}
impl From<AuthError> for WorkspaceError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}
impl From<sqlx::Error> for WorkspaceError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl IntoResponse for WorkspaceError {
    fn into_response(self) -> Response {
        let (s, c, m) = match self {
            Self::Auth(e) => return e.into_response(),
            Self::Invalid(m) => (StatusCode::BAD_REQUEST, "invalid_workspace", m),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "workspace_not_found",
                "작업공간을 찾을 수 없습니다",
            ),
            Self::Conflict(m) => (StatusCode::CONFLICT, "workspace_conflict", m),
            Self::Database(e) => {
                tracing::error!(%e,"작업공간 처리 실패");
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

fn hash_json(value: &Value) -> Result<Vec<u8>, WorkspaceError> {
    Ok(Sha256::digest(
        serde_json::to_vec(value)
            .map_err(|_| WorkspaceError::Invalid("입력을 처리할 수 없습니다"))?,
    )
    .to_vec())
}
pub fn sign_receipt(secret: &[u8], receipt_hash: &[u8]) -> Vec<u8> {
    let mut key = [0_u8; 64];
    if secret.len() > key.len() {
        key[..32].copy_from_slice(&Sha256::digest(secret));
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    let mut inner = [0x36_u8; 64];
    let mut outer = [0x5c_u8; 64];
    for index in 0..64 {
        inner[index] ^= key[index];
        outer[index] ^= key[index];
    }
    let inner_hash = Sha256::new()
        .chain_update(inner)
        .chain_update(receipt_hash)
        .finalize();
    Sha256::new()
        .chain_update(outer)
        .chain_update(inner_hash)
        .finalize()
        .to_vec()
}
pub fn verify_receipt(secret: &[u8], receipt_hash: &[u8], signature: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    sign_receipt(secret, receipt_hash)
        .as_slice()
        .ct_eq(signature)
        .into()
}
fn files_json(files: &[WorkspaceFileInput]) -> Value {
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    json!(files)
}
fn content_hash(value: Option<&String>) -> Option<String> {
    value.map(|text| {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    })
}
fn semantic_file_hash(files: &[(String, String)]) -> Vec<u8> {
    let canonical = files
        .iter()
        .map(|(path, content)| {
            let code = content
                .lines()
                .map(|line| {
                    let line = line.split("//").next().unwrap_or("");
                    let line = line.split('#').next().unwrap_or("");
                    line.split_whitespace().collect::<String>()
                })
                .collect::<String>();
            format!("{path}:{code}")
        })
        .collect::<String>();
    Sha256::digest(canonical.as_bytes()).to_vec()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRequest {
    title: String,
    template_slug: String,
    project_id: Option<Uuid>,
    files: Vec<WorkspaceFileInput>,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveWorkspaceRequest {
    files: Vec<WorkspaceFileInput>,
    expected_version: i32,
    idempotency_key: Uuid,
}
fn baseline() -> String {
    "baseline".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    idempotency_key: Uuid,
    expected_version: i32,
    #[serde(default = "baseline")]
    validation_kind: String,
    challenge_id: Option<Uuid>,
}

async fn replace_files(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: Uuid,
    user: Uuid,
    files: &[WorkspaceFileInput],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM workspace_files WHERE workspace_id=$1 AND user_id=$2")
        .bind(workspace)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    for file in files {
        sqlx::query("INSERT INTO workspace_files(workspace_id,user_id,path,path_key,content,content_hash) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(workspace).bind(user).bind(&file.path).bind(file.path.to_lowercase()).bind(&file.content).bind(Sha256::digest(file.content.as_bytes()).to_vec()).execute(&mut **tx).await?;
    }
    Ok(())
}
async fn workspace_json(
    pool: &sqlx::PgPool,
    user: Uuid,
    id: Uuid,
) -> Result<Value, WorkspaceError> {
    let row:Option<(Uuid,String,i32,String,String,String,Value)>=sqlx::query_as("SELECT w.id,w.title,w.version,t.slug,t.track_kind,t.runtime_status,COALESCE(jsonb_agg(jsonb_build_object('path',f.path,'content',f.content) ORDER BY f.path) FILTER(WHERE f.path IS NOT NULL),'[]') FROM project_workspaces w JOIN workspace_templates t ON(t.id,t.revision,t.template_digest)=(w.template_id,w.template_revision,w.template_digest) LEFT JOIN workspace_files f ON(f.workspace_id,f.user_id)=(w.id,w.user_id) WHERE w.id=$1 AND w.user_id=$2 GROUP BY w.id,t.slug,t.track_kind,t.runtime_status")
        .bind(id).bind(user).fetch_optional(pool).await?;
    row.map(|r|json!({"id":r.0,"title":r.1,"version":r.2,"template_slug":r.3,"track_kind":r.4,"runtime_status":r.5,"files":r.6})).ok_or(WorkspaceError::NotFound)
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<Value>), WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.title.trim().is_empty()
        || req.title.chars().count() > 120
        || !req
            .template_slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(WorkspaceError::Invalid("제목과 템플릿을 확인해 주세요"));
    }
    validate_files(&req.files).map_err(WorkspaceError::Invalid)?;
    let canonical = json!({"title":req.title.trim(),"template_slug":req.template_slug,"project_id":req.project_id,"files":files_json(&req.files)});
    let request_hash = hash_json(&canonical)?;
    let mut tx = state.pool().begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some((id, old)) = sqlx::query_as::<_, (Uuid, Vec<u8>)>(
        "SELECT id,request_hash FROM project_workspaces WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(user)
    .bind(req.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if old != request_hash {
            return Err(WorkspaceError::Conflict(
                "같은 멱등키에 다른 작업공간을 사용할 수 없습니다",
            ));
        }
        tx.commit().await?;
        return Ok((
            StatusCode::OK,
            Json(workspace_json(state.pool(), user, id).await?),
        ));
    }
    let template:Option<(Uuid,i32,Vec<u8>)>=sqlx::query_as("SELECT id,revision,template_digest FROM workspace_templates WHERE slug=$1 ORDER BY revision DESC LIMIT 1").bind(&req.template_slug).fetch_optional(&mut *tx).await?;
    let template = template.ok_or(WorkspaceError::Invalid("지원되는 템플릿이 아닙니다"))?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO project_workspaces(id,user_id,project_id,template_id,template_revision,template_digest,title,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(user).bind(req.project_id).bind(template.0).bind(template.1).bind(template.2).bind(req.title.trim()).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    replace_files(&mut tx, id, user, &req.files).await?;
    let artifact = files_json(&req.files);
    let artifact_hash = hash_json(&artifact)?;
    sqlx::query("INSERT INTO workspace_revisions(workspace_id,user_id,version,artifact,artifact_hash) VALUES($1,$2,1,$3,$4)").bind(id).bind(user).bind(SqlJson(artifact)).bind(artifact_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(workspace_json(state.pool(), user, id).await?),
    ))
}
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM project_workspaces WHERE user_id=$1 ORDER BY created_at DESC",
    )
    .bind(user)
    .fetch_all(state.pool())
    .await?;
    let mut rows = Vec::new();
    for id in ids {
        rows.push(workspace_json(state.pool(), user, id).await?);
    }
    Ok(Json(json!({"workspaces":rows})))
}
pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    Ok(Json(workspace_json(state.pool(), user, id).await?))
}
pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
    Json(req): Json<SaveWorkspaceRequest>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    validate_files(&req.files).map_err(WorkspaceError::Invalid)?;
    let hash = hash_json(
        &json!({"action":"save","workspace_id":id,"expected_version":req.expected_version,"files":files_json(&req.files)}),
    )?;
    let mut tx = state.pool().begin().await?;
    let version: Option<i32> = sqlx::query_scalar(
        "SELECT version FROM project_workspaces WHERE id=$1 AND user_id=$2 AND status='active' AND expires_at>now() FOR UPDATE",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    let version = version.ok_or(WorkspaceError::NotFound)?;
    if let Some((old,result,old_workspace))=sqlx::query_as::<_,(Vec<u8>,i32,Uuid)>("SELECT request_hash,resulting_version,workspace_id FROM workspace_mutations WHERE user_id=$1 AND idempotency_key=$2").bind(user).bind(req.idempotency_key).fetch_optional(&mut *tx).await?{
        if old_workspace!=id{return Err(WorkspaceError::Conflict("같은 멱등키를 다른 작업공간에 사용할 수 없습니다"));}
        if old!=hash{return Err(WorkspaceError::Conflict("같은 멱등키에 다른 파일을 사용할 수 없습니다"));}
        if result!=version{return Err(WorkspaceError::Conflict("재생 결과가 현재 버전과 다릅니다"));}
        tx.commit().await?;return Ok(Json(workspace_json(state.pool(),user,id).await?));
    }
    if version != req.expected_version {
        return Err(WorkspaceError::Conflict("작업공간이 이미 변경되었습니다"));
    }
    replace_files(&mut tx, id, user, &req.files).await?;
    let next = version + 1;
    sqlx::query(
        "UPDATE project_workspaces SET version=$3,updated_at=now() WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user)
    .bind(next)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO workspace_mutations(workspace_id,user_id,idempotency_key,request_hash,resulting_version) VALUES($1,$2,$3,$4,$5)").bind(id).bind(user).bind(req.idempotency_key).bind(hash).bind(next).execute(&mut *tx).await?;
    let artifact = files_json(&req.files);
    let artifact_hash = hash_json(&artifact)?;
    sqlx::query("INSERT INTO workspace_revisions(workspace_id,user_id,version,artifact,artifact_hash) VALUES($1,$2,$3,$4,$5)").bind(id).bind(user).bind(next).bind(SqlJson(artifact)).bind(artifact_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(workspace_json(state.pool(), user, id).await?))
}
pub async fn create_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
    Json(req): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<Value>), WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !matches!(
        req.validation_kind.as_str(),
        "baseline" | "modification" | "transfer"
    ) || (req.validation_kind == "baseline") != req.challenge_id.is_none()
    {
        return Err(WorkspaceError::Invalid(
            "검증 실행 유형과 이해 과제 연결을 확인해 주세요",
        ));
    }
    let mut tx = state.pool().begin().await?;
    let template:Option<(i32,Uuid,i32,Vec<u8>,String,SqlJson<Value>,String,Vec<u8>,bool,String)>=sqlx::query_as("SELECT w.version,w.template_id,w.template_revision,w.template_digest,t.image_reference,t.run_command,t.check_suite_id,t.check_suite_hash,t.supports_tests,t.runtime_status FROM project_workspaces w JOIN workspace_templates t ON(t.id,t.revision,t.template_digest)=(w.template_id,w.template_revision,w.template_digest) WHERE w.id=$1 AND w.user_id=$2 AND w.status='active' AND w.expires_at>now() FOR UPDATE OF w").bind(id).bind(user).fetch_optional(&mut *tx).await?;
    let (
        version,
        tid,
        trev,
        tdigest,
        image,
        command,
        suite,
        suite_hash,
        supports_tests,
        runtime_status,
    ) = template.ok_or(WorkspaceError::NotFound)?;
    if runtime_status != "verified" {
        return Err(WorkspaceError::Conflict(
            "이 템플릿 런타임은 아직 검증되지 않아 실행할 수 없습니다",
        ));
    }
    let request_hash = hash_json(
        &json!({"workspace_id":id,"expected_version":req.expected_version,"validation_kind":req.validation_kind,"challenge_id":req.challenge_id}),
    )?;
    if let Some(challenge) = req.challenge_id {
        let source: (Uuid,Vec<u8>,Vec<u8>,Option<time::OffsetDateTime>) = sqlx::query_as(
            "SELECT workspace_id,source_template_digest,check_suite_hash,predictions_committed_at FROM understanding_challenges WHERE id=$1 AND user_id=$2",
        )
        .bind(challenge)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkspaceError::Invalid("소유한 이해 과제가 아닙니다"))?;
        if source.3.is_none() {
            return Err(WorkspaceError::Conflict(
                "변형·전이 실행 전에 결과 예측을 먼저 확정해야 합니다",
            ));
        }
        if (req.validation_kind == "modification"
            && (source.0 != id || source.1 != tdigest || source.2 != suite_hash))
            || (req.validation_kind == "transfer"
                && (source.0 == id || source.1 == tdigest || source.2 == suite_hash))
        {
            return Err(WorkspaceError::Invalid(
                "변형은 원 템플릿, 전이는 다른 작업공간·템플릿·검사 묶음에서 실행해야 합니다",
            ));
        }
    }
    if let Some((run, status, old_hash)) = sqlx::query_as::<_, (Uuid, String, Vec<u8>)>(
        "SELECT id,status,request_hash FROM workspace_runs WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(user)
    .bind(req.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if old_hash != request_hash {
            return Err(WorkspaceError::Conflict(
                "같은 멱등키에 다른 실행 요청을 사용할 수 없습니다",
            ));
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(json!({"id":run,"status":status}))));
    }
    if version != req.expected_version {
        return Err(WorkspaceError::Conflict(
            "실행할 작업공간 버전이 이미 변경되었습니다",
        ));
    }
    let files:Vec<(String,String)>=sqlx::query_as("SELECT path,content FROM workspace_files WHERE workspace_id=$1 AND user_id=$2 ORDER BY path").bind(id).bind(user).fetch_all(&mut *tx).await?;
    let artifact = json!(
        files
            .iter()
            .map(|f| json!({"path":f.0,"content":f.1}))
            .collect::<Vec<_>>()
    );
    let ah = hash_json(&artifact)?;
    let semantic_hash = semantic_file_hash(&files);
    let run = Uuid::now_v7();
    sqlx::query("INSERT INTO workspace_runs(id,workspace_id,user_id,workspace_version,template_id,template_revision,template_digest,image_reference,artifact,artifact_hash,semantic_hash,validation_kind,challenge_id,check_suite_id,check_suite_hash,supports_tests_snapshot,command,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)").bind(run).bind(id).bind(user).bind(version).bind(tid).bind(trev).bind(tdigest).bind(image).bind(SqlJson(artifact)).bind(ah).bind(semantic_hash).bind(req.validation_kind).bind(req.challenge_id).bind(suite).bind(suite_hash).bind(supports_tests).bind(command).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":run,"status":"queued"})),
    ))
}
pub async fn run_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let r:Option<(Uuid,String,Option<i32>,Option<String>,Option<String>,bool,bool)>=sqlx::query_as("SELECT id,status,exit_code,stdout,stderr,output_truncated,cancel_requested_at IS NOT NULL FROM workspace_runs WHERE id=$1 AND user_id=$2").bind(id).bind(user).fetch_optional(state.pool()).await?;
    let r = r.ok_or(WorkspaceError::NotFound)?;
    Ok(Json(
        json!({"id":r.0,"status":r.1,"exit_code":r.2,"stdout":r.3,"stderr":r.4,"output_truncated":r.5,"cancel_requested":r.6}),
    ))
}
pub async fn cancel_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let r=sqlx::query("UPDATE workspace_runs SET status=CASE WHEN status='queued' THEN 'cancelled' ELSE status END,cancel_requested_at=now(),completed_at=CASE WHEN status='queued' THEN now() ELSE completed_at END WHERE id=$1 AND user_id=$2 AND status IN('queued','leased','running')").bind(id).bind(user).execute(state.pool()).await?;
    if r.rows_affected() == 0 {
        return Err(WorkspaceError::NotFound);
    }
    Ok(Json(json!({"id":id,"cancel_requested":true})))
}

pub struct LeasedWorkspaceRun {
    pub id: Uuid,
    pub lease_token: Uuid,
    pub image: String,
    pub files: Vec<WorkspaceFileInput>,
    pub command: Vec<String>,
}
pub async fn lease_next(
    pool: &sqlx::PgPool,
    worker: &str,
    execution_image_digest: &str,
) -> Result<Option<LeasedWorkspaceRun>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE workspace_runs SET status='expired',lease_token=NULL,leased_by=NULL,lease_expires_at=NULL,completed_at=now(),stderr='최대 재시도 횟수를 초과했습니다' WHERE status IN('leased','running') AND lease_expires_at<now() AND attempt>=3").execute(&mut *tx).await?;
    let row:Option<(Uuid,String,SqlJson<Value>,SqlJson<Value>)>=sqlx::query_as("WITH candidate AS(SELECT r.id FROM workspace_runs r JOIN project_workspaces w ON(w.id,w.user_id)=(r.workspace_id,r.user_id) WHERE w.status='active' AND w.expires_at>now() AND (r.status='queued' OR(r.status IN('leased','running') AND r.lease_expires_at<now() AND r.attempt<3)) ORDER BY r.created_at FOR UPDATE OF r SKIP LOCKED LIMIT 1) UPDATE workspace_runs r SET status='leased',attempt=attempt+1,lease_token=gen_random_uuid(),leased_by=$1,lease_expires_at=now()+interval '30 seconds',execution_image_digest=$2 FROM candidate WHERE r.id=candidate.id RETURNING r.id,r.image_reference,r.artifact,r.command").bind(worker).bind(execution_image_digest).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let lease: Uuid = sqlx::query_scalar("SELECT lease_token FROM workspace_runs WHERE id=$1")
        .bind(row.0)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let files: Vec<WorkspaceFileInput> =
        serde_json::from_value(row.2.0).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    let command: Vec<String> =
        serde_json::from_value(row.3.0).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    Ok(Some(LeasedWorkspaceRun {
        id: row.0,
        lease_token: lease,
        image: row.1,
        files,
        command,
    }))
}
pub async fn mark_running(pool: &sqlx::PgPool, id: Uuid, token: Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE workspace_runs SET status='running' WHERE id=$1 AND lease_token=$2 AND status='leased' AND lease_expires_at>now() AND cancel_requested_at IS NULL").bind(id).bind(token).execute(pool).await?.rows_affected()==1)
}
pub async fn complete_run(
    pool: &sqlx::PgPool,
    id: Uuid,
    token: Uuid,
    out: &crate::sandbox::WorkspaceOutcome,
    receipt_secret: &[u8],
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row:Option<(Vec<u8>,Vec<u8>,String,Option<Uuid>,Vec<u8>,String,SqlJson<Value>,String,Vec<u8>,String,bool)>=sqlx::query_as("SELECT r.artifact_hash,r.semantic_hash,r.validation_kind,r.challenge_id,r.template_digest,r.image_reference,r.command,r.check_suite_id,r.check_suite_hash,r.execution_image_digest,r.supports_tests_snapshot FROM workspace_runs r JOIN project_workspaces w ON(w.id,w.user_id)=(r.workspace_id,r.user_id) WHERE r.id=$1 AND r.lease_token=$2 AND r.status='running' AND r.lease_expires_at>now() AND r.cancel_requested_at IS NULL AND w.status='active' AND w.expires_at>now() FOR UPDATE OF r").bind(id).bind(token).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    let stdout_hash = Sha256::digest(out.stdout.as_bytes()).to_vec();
    let passed = out.status == "succeeded" && out.exit_code == Some(0) && !out.output_truncated;
    let receipt=Sha256::digest(serde_json::to_vec(&json!({"run":id,"lease_token":token,"artifact_hash":row.0,"semantic_hash":row.1,"validation_kind":row.2,"challenge_id":row.3,"template_digest":row.4,"declared_image_reference":row.5,"command":row.6.0,"check_suite_id":row.7,"check_suite_hash":row.8,"execution_image_digest":row.9,"supports_tests":row.10,"status":out.status,"exit_code":out.exit_code,"stdout_hash":stdout_hash,"output_truncated":out.output_truncated,"checks_passed":passed})).unwrap_or_default()).to_vec();
    let receipt_mac = sign_receipt(receipt_secret, &receipt);
    sqlx::query("UPDATE workspace_runs SET status=$3,lease_token=NULL,leased_by=NULL,lease_expires_at=NULL,exit_code=$4,stdout=$5,stderr=$6,output_truncated=$7,stdout_hash=$8,deterministic_checks_passed=$9,runner_receipt_hash=$10,runner_receipt_mac=$11,completed_at=now() WHERE id=$1 AND lease_token=$2").bind(id).bind(token).bind(out.status).bind(out.exit_code).bind(&out.stdout).bind(&out.stderr).bind(out.output_truncated).bind(stdout_hash).bind(passed).bind(receipt).bind(receipt_mac).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}
pub async fn run_cancelled(
    pool: &sqlx::PgPool,
    id: Uuid,
    token: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT cancel_requested_at IS NOT NULL OR lease_expires_at<=now() FROM workspace_runs WHERE id=$1 AND lease_token=$2 AND status='running'").bind(id).bind(token).fetch_optional(pool).await.map(|v|v.unwrap_or(true))
}
pub async fn finish_cancel(pool: &sqlx::PgPool, id: Uuid, token: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE workspace_runs SET status=CASE WHEN cancel_requested_at IS NOT NULL THEN 'cancelled' ELSE 'queued' END,lease_token=NULL,leased_by=NULL,lease_expires_at=NULL,completed_at=CASE WHEN cancel_requested_at IS NOT NULL THEN now() ELSE NULL END WHERE id=$1 AND lease_token=$2 AND status IN('leased','running')").bind(id).bind(token).execute(pool).await?;
    Ok(())
}
pub const WORKSPACE_LEASE: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    name: String,
    idempotency_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    version: i32,
    expected_version: i32,
    idempotency_key: Uuid,
}

pub async fn revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath((id, version)): AxumPath<(Uuid, i32)>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id(&state, &headers).await?;
    let row:Option<(SqlJson<Value>,Vec<u8>)>=sqlx::query_as("SELECT artifact,artifact_hash FROM workspace_revisions WHERE workspace_id=$1 AND user_id=$2 AND version=$3").bind(id).bind(user).bind(version).fetch_optional(state.pool()).await?;
    let (artifact, hash) = row.ok_or(WorkspaceError::NotFound)?;
    let previous:Option<SqlJson<Value>>=sqlx::query_scalar("SELECT artifact FROM workspace_revisions WHERE workspace_id=$1 AND user_id=$2 AND version<$3 ORDER BY version DESC LIMIT 1").bind(id).bind(user).bind(version).fetch_optional(state.pool()).await?;
    let map = |value: &Value| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f["path"].as_str()?.to_owned(),
                    f["content"].as_str()?.to_owned(),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let current = map(&artifact.0);
    let prior = previous.as_ref().map(|v| map(&v.0)).unwrap_or_default();
    let mut changed = current
        .keys()
        .chain(prior.keys())
        .filter(|path| current.get(*path) != prior.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    let changes=changed.iter().map(|path|json!({"path":path,"kind":match(current.contains_key(path),prior.contains_key(path)){(true,false)=>"added",(false,true)=>"deleted",_=>"modified"},"old_hash":content_hash(prior.get(path)),"new_hash":content_hash(current.get(path))})).collect::<Vec<_>>();
    Ok(Json(
        json!({"workspace_id":id,"version":version,"artifact":artifact.0,"artifact_hash":hash,"changed_paths":changed,"changes":changes}),
    ))
}
pub async fn checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
    Json(req): Json<CheckpointRequest>,
) -> Result<(StatusCode, Json<Value>), WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if req.name.trim().is_empty() || req.name.chars().count() > 80 {
        return Err(WorkspaceError::Invalid("체크포인트 이름을 확인해 주세요"));
    }
    let request_hash =
        hash_json(&json!({"action":"checkpoint","workspace_id":id,"name":req.name.trim()}))?;
    let mut tx = state.pool().begin().await?;
    let version:Option<i32>=sqlx::query_scalar("SELECT version FROM project_workspaces WHERE id=$1 AND user_id=$2 AND status='active' AND expires_at>now() FOR UPDATE").bind(id).bind(user).fetch_optional(&mut *tx).await?;
    let version = version.ok_or(WorkspaceError::NotFound)?;
    if let Some((existing,old,version,name))=sqlx::query_as::<_,(Uuid,Vec<u8>,i32,String)>("SELECT id,request_hash,version,name FROM workspace_checkpoints WHERE user_id=$1 AND idempotency_key=$2").bind(user).bind(req.idempotency_key).fetch_optional(&mut *tx).await?{if old!=request_hash{return Err(WorkspaceError::Conflict("같은 멱등키에 다른 체크포인트를 사용할 수 없습니다"));}tx.commit().await?;return Ok((StatusCode::OK,Json(json!({"id":existing,"version":version,"name":name}))));}
    let checkpoint = Uuid::now_v7();
    sqlx::query("INSERT INTO workspace_checkpoints(id,workspace_id,user_id,version,name,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(checkpoint).bind(id).bind(user).bind(version).bind(req.name.trim()).bind(req.idempotency_key).bind(request_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":checkpoint,"version":version,"name":req.name.trim()})),
    ))
}
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<Uuid>,
    Json(req): Json<ResetRequest>,
) -> Result<Json<Value>, WorkspaceError> {
    let user = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let mut tx = state.pool().begin().await?;
    let request_hash = hash_json(
        &json!({"action":"reset","workspace_id":id,"version":req.version,"expected_version":req.expected_version}),
    )?;
    let current:Option<i32>=sqlx::query_scalar("SELECT version FROM project_workspaces WHERE id=$1 AND user_id=$2 AND status='active' AND expires_at>now() FOR UPDATE").bind(id).bind(user).fetch_optional(&mut *tx).await?;
    let current = current.ok_or(WorkspaceError::NotFound)?;
    if let Some((old,result,old_workspace))=sqlx::query_as::<_,(Vec<u8>,i32,Uuid)>("SELECT request_hash,resulting_version,workspace_id FROM workspace_mutations WHERE user_id=$1 AND idempotency_key=$2").bind(user).bind(req.idempotency_key).fetch_optional(&mut *tx).await?{
        if old_workspace!=id||old!=request_hash{return Err(WorkspaceError::Conflict("같은 멱등키에 다른 초기화를 사용할 수 없습니다"));}
        if result!=current{return Err(WorkspaceError::Conflict("초기화 재생 버전이 다릅니다"));}
        tx.commit().await?;return Ok(Json(workspace_json(state.pool(),user,id).await?));
    }
    if current != req.expected_version {
        return Err(WorkspaceError::Conflict("작업공간이 이미 변경되었습니다"));
    }
    let artifact:Option<SqlJson<Value>>=sqlx::query_scalar("SELECT artifact FROM workspace_revisions WHERE workspace_id=$1 AND user_id=$2 AND version=$3").bind(id).bind(user).bind(req.version).fetch_optional(&mut *tx).await?;
    let artifact = artifact.ok_or(WorkspaceError::NotFound)?;
    let files: Vec<WorkspaceFileInput> = serde_json::from_value(artifact.0)
        .map_err(|_| WorkspaceError::Invalid("저장된 리비전을 읽을 수 없습니다"))?;
    replace_files(&mut tx, id, user, &files).await?;
    let next = current + 1;
    sqlx::query(
        "UPDATE project_workspaces SET version=$3,updated_at=now() WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user)
    .bind(next)
    .execute(&mut *tx)
    .await?;
    let new_artifact = files_json(&files);
    let new_hash = hash_json(&new_artifact)?;
    sqlx::query("INSERT INTO workspace_revisions(workspace_id,user_id,version,artifact,artifact_hash) VALUES($1,$2,$3,$4,$5)").bind(id).bind(user).bind(next).bind(SqlJson(new_artifact)).bind(new_hash).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO workspace_mutations(workspace_id,user_id,idempotency_key,request_hash,resulting_version) VALUES($1,$2,$3,$4,$5)").bind(id).bind(user).bind(req.idempotency_key).bind(request_hash).bind(next).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(workspace_json(state.pool(), user, id).await?))
}
pub async fn expire(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let ids:Vec<Uuid>=sqlx::query_scalar("UPDATE project_workspaces SET status='expired' WHERE status='active' AND expires_at<=now() RETURNING id").fetch_all(&mut *tx).await?;
    let count=sqlx::query("UPDATE workspace_runs SET status=CASE WHEN status='queued' THEN 'expired' ELSE status END,cancel_requested_at=CASE WHEN status IN('leased','running') THEN now() ELSE cancel_requested_at END,completed_at=CASE WHEN status='queued' THEN now() ELSE completed_at END WHERE workspace_id=ANY($1) AND status IN('queued','leased','running')").bind(&ids).execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;
    Ok(count)
}
