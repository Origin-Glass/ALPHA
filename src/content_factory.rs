use std::net::IpAddr;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Host, Url};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum ContentFactoryError {
    Auth(AuthError),
    InvalidInput(&'static str),
    Forbidden,
    NotFound,
    Conflict(&'static str),
    Database(sqlx::Error),
}

impl IntoResponse for ContentFactoryError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "content_factory_forbidden",
                "콘텐츠 생성 권한이 없습니다",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "content_factory_not_found",
                "대상을 찾을 수 없습니다",
            ),
            Self::Conflict(message) => (StatusCode::CONFLICT, "content_factory_conflict", message),
            Self::Database(error) => {
                tracing::error!(%error, "콘텐츠 공장 데이터베이스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
        };
        (
            status,
            Json(serde_json::json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

impl From<AuthError> for ContentFactoryError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
impl From<sqlx::Error> for ContentFactoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    capability: &str,
) -> Result<Uuid, ContentFactoryError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(state, headers).await?;
    crate::auth::require_current_policy(state, user_id).await?;
    if !crate::governance::has_capability(state.pool(), user_id, capability).await? {
        return Err(ContentFactoryError::Forbidden);
    }
    Ok(user_id)
}

fn external_ip_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || octets[0] >= 240
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| external_ip_forbidden(IpAddr::V4(mapped)))
        }
    }
}

pub fn validate_provider_url(kind: &str, raw: &str) -> Result<Url, ContentFactoryError> {
    let url = Url::parse(raw)
        .map_err(|_| ContentFactoryError::InvalidInput("제공자 URL을 확인해 주세요"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ContentFactoryError::InvalidInput(
            "제공자 URL은 사용자정보 없는 HTTPS 주소여야 합니다",
        ));
    }
    let host = url.host().ok_or(ContentFactoryError::InvalidInput(
        "제공자 호스트가 필요합니다",
    ))?;
    match kind {
        "local" => {
            let loopback = matches!(host, Host::Ipv4(ip) if ip.is_loopback())
                || matches!(host, Host::Ipv6(ip) if ip.is_loopback());
            if !loopback || url.port() != Some(11434) {
                return Err(ContentFactoryError::InvalidInput(
                    "로컬 제공자는 HTTPS 루프백 11434 포트만 허용합니다",
                ));
            }
        }
        "external" => {
            if url.port_or_known_default() != Some(443) {
                return Err(ContentFactoryError::InvalidInput(
                    "외부 제공자는 HTTPS 443 포트만 허용합니다",
                ));
            }
            match host {
                Host::Ipv4(ip) if external_ip_forbidden(IpAddr::V4(ip)) => {
                    return Err(ContentFactoryError::InvalidInput(
                        "사설 네트워크 제공자는 허용되지 않습니다",
                    ));
                }
                Host::Ipv6(ip) if external_ip_forbidden(IpAddr::V6(ip)) => {
                    return Err(ContentFactoryError::InvalidInput(
                        "사설 네트워크 제공자는 허용되지 않습니다",
                    ));
                }
                Host::Domain(domain)
                    if domain == "localhost"
                        || domain.ends_with(".localhost")
                        || domain.ends_with(".local")
                        || domain.ends_with(".internal") =>
                {
                    return Err(ContentFactoryError::InvalidInput(
                        "로컬 도메인은 외부 제공자로 사용할 수 없습니다",
                    ));
                }
                _ => {}
            }
        }
        _ => {
            return Err(ContentFactoryError::InvalidInput(
                "제공자 종류를 확인해 주세요",
            ));
        }
    }
    Ok(url)
}

fn credential_name_allowed(name: &str) -> bool {
    matches!(
        name,
        "CONTENT_AI_CUSTOM_API_KEY" | "OPENROUTER_API_KEY" | "ANTHROPIC_API_KEY" | "OPENAI_API_KEY"
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequest {
    name: String,
    kind: String,
    base_url: String,
    model: String,
    credential_env_var: Option<String>,
    enabled: bool,
}

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ProviderRequest>,
) -> Result<(StatusCode, Json<Value>), ContentFactoryError> {
    let user_id = authorize(&state, &headers, "provider.configure").await?;
    validate_provider_url(&request.kind, &request.base_url)?;
    if request.name.len() < 3
        || request.name.len() > 40
        || !request.name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'_' | b'-'))
        })
        || request.model.trim().is_empty()
        || request.model.len() > 120
        || (request.kind == "external"
            && !request
                .credential_env_var
                .as_deref()
                .is_some_and(credential_name_allowed))
        || (request.kind == "local" && request.credential_env_var.is_some())
    {
        return Err(ContentFactoryError::InvalidInput(
            "제공자 설정을 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO content_provider_configs
           (name, kind, base_url, model, credential_env_var, enabled, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id"#,
    )
    .bind(&request.name)
    .bind(&request.kind)
    .bind(&request.base_url)
    .bind(request.model.trim())
    .bind(&request.credential_env_var)
    .bind(request.enabled)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_provider.created', 'content_provider', $2, jsonb_build_object('kind', $3::text))")
        .bind(user_id).bind(id.to_string()).bind(&request.kind).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateJobRequest {
    provider_id: Uuid,
    content_type: String,
    topic: String,
    target_language: String,
    generation_count: i16,
    estimated_cost_microunits: i64,
}

pub async fn create_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<Value>), ContentFactoryError> {
    let user_id = authorize(&state, &headers, "content.generate").await?;
    if !matches!(
        request.content_type.as_str(),
        "algorithm_problem"
            | "code_reading"
            | "debugging"
            | "documentation_lesson"
            | "implementation_task"
    ) || !(2..=200).contains(&request.topic.trim().chars().count())
        || request.target_language != "ko"
        || !(1..=20).contains(&request.generation_count)
        || !(0..=1_000_000_000).contains(&request.estimated_cost_microunits)
    {
        return Err(ContentFactoryError::InvalidInput(
            "생성 작업 설정을 확인해 주세요",
        ));
    }
    let provider: Option<(bool, Option<String>)> = sqlx::query_as(
        "SELECT enabled, credential_env_var FROM content_provider_configs WHERE id = $1",
    )
    .bind(request.provider_id)
    .fetch_optional(state.pool())
    .await?;
    let (enabled, credential_env_var) = provider.ok_or(ContentFactoryError::NotFound)?;
    let status = if !state.settings().content_ai_enabled || !enabled {
        "blocked_disabled"
    } else if credential_env_var
        .as_deref()
        .is_some_and(|name| !state.settings().has_content_ai_credential(name))
    {
        "blocked_missing_credential"
    } else {
        "queued"
    };
    let spec = serde_json::to_value(&request)
        .map_err(|_| ContentFactoryError::InvalidInput("생성 작업 설정을 확인해 주세요"))?;
    let request_hash = Sha256::digest(serde_json::to_vec(&spec).unwrap_or_default()).to_vec();
    let mut transaction = state.pool().begin().await?;
    let reserved = if status == "queued" {
        let updated = sqlx::query(
            r#"UPDATE content_budgets
               SET reserved_microunits = reserved_microunits + $1, updated_at = now()
               WHERE budget_key = 'global'
                 AND reserved_microunits + spent_microunits + $1 <= limit_microunits"#,
        )
        .bind(request.estimated_cost_microunits)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ContentFactoryError::Conflict(
                "콘텐츠 생성 예산이 부족합니다",
            ));
        }
        request.estimated_cost_microunits
    } else {
        0
    };
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO content_generation_jobs
           (created_by, provider_id, content_type, request_spec, request_hash, status,
            estimated_cost_microunits, reserved_cost_microunits)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id"#,
    )
    .bind(user_id)
    .bind(request.provider_id)
    .bind(&request.content_type)
    .bind(&spec)
    .bind(request_hash)
    .bind(status)
    .bind(request.estimated_cost_microunits)
    .bind(reserved)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_job.created', 'content_generation_job', $2, jsonb_build_object('status', $3::text))")
        .bind(user_id).bind(id.to_string()).bind(status).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "status": status})),
    ))
}
