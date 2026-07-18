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

type ProviderRow = (
    Uuid,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    bool,
);
type JobRow = (
    Uuid,
    String,
    String,
    String,
    i16,
    Option<String>,
    time::OffsetDateTime,
);
type LeaseCandidate = (
    Uuid,
    i16,
    String,
    String,
    String,
    String,
    Option<String>,
    Value,
    Vec<u8>,
    i64,
);

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

async fn authorize_read(
    state: &AppState,
    headers: &HeaderMap,
    capability: &str,
) -> Result<Uuid, ContentFactoryError> {
    let user_id = crate::auth::authenticated_user_id(state, headers).await?;
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
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ContentFactoryError::InvalidInput(
            "제공자 URL에는 사용자정보·쿼리·프래그먼트를 넣을 수 없습니다",
        ));
    }
    let host = url.host().ok_or(ContentFactoryError::InvalidInput(
        "제공자 호스트가 필요합니다",
    ))?;
    match kind {
        "local" => {
            let loopback = matches!(host, Host::Ipv4(ip) if ip == std::net::Ipv4Addr::LOCALHOST)
                || matches!(host, Host::Ipv6(ip) if ip == std::net::Ipv6Addr::LOCALHOST);
            if url.scheme() != "http" || !loopback || url.port() != Some(11434) {
                return Err(ContentFactoryError::InvalidInput(
                    "로컬 제공자는 HTTP 루프백 11434 포트만 허용합니다",
                ));
            }
        }
        "external" => {
            if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
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

#[derive(Debug)]
pub struct ResolvedProviderEndpoint {
    pub url: Url,
    pub dns_pin: Option<(String, std::net::SocketAddr)>,
}

pub async fn resolve_provider_endpoint(
    kind: &str,
    raw: &str,
) -> Result<ResolvedProviderEndpoint, ContentFactoryError> {
    let url = validate_provider_url(kind, raw)?;
    let host = url.host().ok_or(ContentFactoryError::InvalidInput(
        "제공자 호스트가 필요합니다",
    ))?;
    let port = url.port_or_known_default().unwrap_or(443);
    match host {
        Host::Ipv4(ip) => Ok(ResolvedProviderEndpoint {
            url,
            dns_pin: Some((ip.to_string(), (ip, port).into())),
        }),
        Host::Ipv6(ip) => Ok(ResolvedProviderEndpoint {
            url,
            dns_pin: Some((ip.to_string(), (ip, port).into())),
        }),
        Host::Domain(domain) => {
            if kind != "external" {
                return Err(ContentFactoryError::InvalidInput(
                    "로컬 제공자는 IP 루프백 주소만 허용합니다",
                ));
            }
            let domain = domain.to_owned();
            let addresses: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((domain.as_str(), 443))
                    .await
                    .map_err(|_| {
                        ContentFactoryError::InvalidInput("제공자 DNS를 확인할 수 없습니다")
                    })?
                    .collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| external_ip_forbidden(address.ip()))
            {
                return Err(ContentFactoryError::InvalidInput(
                    "제공자 DNS가 사설 네트워크를 가리킵니다",
                ));
            }
            Ok(ResolvedProviderEndpoint {
                url,
                dns_pin: Some((domain, addresses[0])),
            })
        }
    }
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
    protocol: String,
    base_url: String,
    model: String,
    cost_per_generation_microunits: i64,
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
        || !(1..=1_000_000_000).contains(&request.cost_per_generation_microunits)
        || !matches!(request.protocol.as_str(), "openai_compatible" | "anthropic")
        || (request.kind == "external"
            && !request
                .credential_env_var
                .as_deref()
                .is_some_and(credential_name_allowed))
        || (request.kind == "local" && request.credential_env_var.is_some())
        || (request.kind == "local" && request.protocol != "openai_compatible")
        || (request.protocol == "anthropic"
            && request.credential_env_var.as_deref() != Some("ANTHROPIC_API_KEY"))
        || (request.protocol == "openai_compatible"
            && request.credential_env_var.as_deref() == Some("ANTHROPIC_API_KEY"))
    {
        return Err(ContentFactoryError::InvalidInput(
            "제공자 설정을 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO content_provider_configs
           (name, kind, protocol, base_url, model, cost_per_generation_microunits,
            credential_env_var, enabled, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id"#,
    )
    .bind(&request.name)
    .bind(&request.kind)
    .bind(&request.protocol)
    .bind(&request.base_url)
    .bind(request.model.trim())
    .bind(request.cost_per_generation_microunits)
    .bind(&request.credential_env_var)
    .bind(request.enabled)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_provider.created', 'content_provider', $2, jsonb_build_object('kind', $3::text, 'protocol', $4::text))")
        .bind(user_id).bind(id.to_string()).bind(&request.kind).bind(&request.protocol).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}

pub async fn list_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    authorize_read(&state, &headers, "content.generate").await?;
    let providers: Vec<ProviderRow> = sqlx::query_as(
        r#"SELECT id, name, kind, protocol, model, cost_per_generation_microunits,
                      credential_env_var, enabled
               FROM content_provider_configs ORDER BY name"#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(serde_json::json!({
        "providers": providers.into_iter().map(|(id, name, kind, protocol, model, cost, credential_env_var, enabled)| serde_json::json!({
            "id": id,
            "name": name,
            "kind": kind,
            "protocol": protocol,
            "model": model,
            "cost_per_generation_microunits": cost,
            "credential_available": credential_env_var.as_deref().is_none_or(|name| state.settings().has_content_ai_credential(name)),
            "enabled": enabled,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateJobRequest {
    provider_id: Uuid,
    content_type: String,
    topic: String,
    target_language: String,
    generation_count: i16,
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
    {
        return Err(ContentFactoryError::InvalidInput(
            "생성 작업 설정을 확인해 주세요",
        ));
    }
    let provider: Option<(bool, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT enabled, kind, credential_env_var, cost_per_generation_microunits FROM content_provider_configs WHERE id = $1",
    )
    .bind(request.provider_id)
    .fetch_optional(state.pool())
    .await?;
    let (enabled, provider_kind, credential_env_var, unit_cost) =
        provider.ok_or(ContentFactoryError::NotFound)?;
    let estimated_cost_microunits = unit_cost
        .checked_mul(i64::from(request.generation_count))
        .ok_or(ContentFactoryError::InvalidInput(
            "제공자 가격을 확인해 주세요",
        ))?;
    let provider_capability = if provider_kind == "local" {
        "provider.use.local"
    } else {
        "provider.use.frontier"
    };
    if !crate::governance::has_capability(state.pool(), user_id, provider_capability).await? {
        return Err(ContentFactoryError::Forbidden);
    }
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
        .bind(estimated_cost_microunits)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ContentFactoryError::Conflict(
                "콘텐츠 생성 예산이 부족합니다",
            ));
        }
        estimated_cost_microunits
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
    .bind(estimated_cost_microunits)
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

pub async fn list_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize_read(&state, &headers, "content.generate").await?;
    let jobs: Vec<JobRow> = sqlx::query_as(
        r#"SELECT job.id, provider.name, job.content_type, job.status, job.attempt_count,
                      job.last_error_code, job.created_at
               FROM content_generation_jobs job
               JOIN content_provider_configs provider ON provider.id = job.provider_id
               WHERE job.created_by = $1
               ORDER BY job.created_at DESC LIMIT 50"#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(
        serde_json::json!({"jobs": jobs.into_iter().map(|(id, provider, content_type, status, attempts, error, created_at)| serde_json::json!({
        "id": id, "provider": provider, "content_type": content_type, "status": status,
        "attempts": attempts, "last_error_code": error, "created_at": created_at,
    })).collect::<Vec<_>>() }),
    ))
}

pub async fn get_budget(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    authorize_read(&state, &headers, "provider.view_cost").await?;
    let (limit, reserved, spent): (i64, i64, i64) = sqlx::query_as(
        "SELECT limit_microunits, reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(state.pool())
    .await?;
    Ok(Json(serde_json::json!({
        "limit_microunits": limit,
        "reserved_microunits": reserved,
        "spent_microunits": spent,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBudgetRequest {
    limit_microunits: i64,
}

pub async fn update_budget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateBudgetRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(&state, &headers, "provider.configure").await?;
    if !(0..=1_000_000_000_000).contains(&request.limit_microunits) {
        return Err(ContentFactoryError::InvalidInput(
            "예산 한도를 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let updated = sqlx::query(
        r#"UPDATE content_budgets SET limit_microunits = $1, updated_at = now()
           WHERE budget_key = 'global' AND reserved_microunits + spent_microunits <= $1"#,
    )
    .bind(request.limit_microunits)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ContentFactoryError::Conflict(
            "예약·사용 금액보다 예산을 낮출 수 없습니다",
        ));
    }
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_budget.updated', 'content_budget', 'global', jsonb_build_object('limit_microunits', $2::bigint))")
        .bind(user_id).bind(request.limit_microunits).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"limit_microunits": request.limit_microunits}),
    ))
}

#[derive(Debug, sqlx::FromRow)]
pub struct GenerationLease {
    pub job_id: Uuid,
    pub attempt_id: Uuid,
    pub attempt_number: i16,
    pub lease_token: Uuid,
    pub provider_kind: String,
    pub provider_protocol: String,
    pub base_url: String,
    pub model: String,
    pub credential_env_var: Option<String>,
    pub request_spec: Value,
    pub reserved_cost_microunits: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CompleteGenerationError {
    #[error("generation lease lost")]
    LeaseLost,
    #[error("actual cost exceeds reservation")]
    CostExceeded,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn lease_next_generation_job(
    pool: &sqlx::PgPool,
    worker_id: &str,
    lease_duration: std::time::Duration,
) -> Result<Option<GenerationLease>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"UPDATE content_generation_attempts attempt
           SET status = 'retryable_failure', error_code = 'lease_expired', completed_at = now()
           FROM content_generation_jobs job
           WHERE attempt.job_id = job.id AND attempt.attempt_number = job.attempt_count
             AND attempt.status = 'running' AND job.status = 'leased' AND job.lease_expires_at <= now()"#,
    )
    .execute(&mut *transaction)
    .await?;

    let expired_leases: Vec<(Uuid, i16, i16, i64)> = sqlx::query_as(
        r#"SELECT id, attempt_count, max_attempts, reserved_cost_microunits
           FROM content_generation_jobs
           WHERE status = 'leased' AND lease_expires_at <= now() AND NOT settled
           FOR UPDATE"#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    let terminal_reserved: i64 = expired_leases
        .iter()
        .filter(|(_, attempt, max_attempts, _)| attempt >= max_attempts)
        .map(|(_, _, _, reserved)| *reserved)
        .sum();
    if terminal_reserved > 0 {
        sqlx::query(
            "UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, updated_at = now() WHERE budget_key = 'global'",
        )
        .bind(terminal_reserved)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"UPDATE content_generation_jobs
           SET status = 'failed', settled = true, reserved_cost_microunits = 0,
               lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
               last_error_code = 'lease_attempts_exhausted', updated_at = now()
           WHERE status = 'leased' AND lease_expires_at <= now() AND attempt_count >= max_attempts"#,
    )
    .execute(&mut *transaction)
    .await?;
    for (job_id, attempt, max_attempts, _) in &expired_leases {
        let outcome = if attempt >= max_attempts {
            "failed"
        } else {
            "requeued"
        };
        sqlx::query(
            "INSERT INTO audit_events (action, target_type, target_id, metadata) VALUES ('content_job.lease_expired', 'content_generation_job', $1, jsonb_build_object('attempt', $2::smallint, 'outcome', $3::text))",
        )
        .bind(job_id.to_string())
        .bind(*attempt)
        .bind(outcome)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"UPDATE content_generation_jobs
           SET status = 'queued', lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
               available_at = now() + make_interval(secs => (1 << LEAST(GREATEST(attempt_count - 1, 0), 6))),
               last_error_code = 'lease_expired', updated_at = now()
           WHERE status = 'leased' AND lease_expires_at <= now() AND attempt_count < max_attempts"#,
    )
    .execute(&mut *transaction)
    .await?;

    let candidate: Option<LeaseCandidate> = sqlx::query_as(
        r#"SELECT job.id, job.attempt_count, provider.kind, provider.protocol, provider.base_url,
                      provider.model, provider.credential_env_var, job.request_spec,
                      job.request_hash, job.reserved_cost_microunits
               FROM content_generation_jobs job
               JOIN content_provider_configs provider ON provider.id = job.provider_id
               WHERE job.status = 'queued' AND job.available_at <= now()
               ORDER BY job.available_at, job.created_at
               FOR UPDATE OF job SKIP LOCKED LIMIT 1"#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((
        job_id,
        attempt_count,
        provider_kind,
        provider_protocol,
        base_url,
        model,
        credential_env_var,
        request_spec,
        request_hash,
        reserved_cost_microunits,
    )) = candidate
    else {
        transaction.commit().await?;
        return Ok(None);
    };
    let attempt_number = attempt_count + 1;
    let lease_token = Uuid::now_v7();
    let attempt_id = Uuid::now_v7();
    let lease_seconds = i64::try_from(lease_duration.as_secs())
        .unwrap_or(i64::MAX)
        .clamp(1, 300);
    let lease_expires_at = time::OffsetDateTime::now_utc() + time::Duration::seconds(lease_seconds);
    sqlx::query(
        r#"UPDATE content_generation_jobs
           SET status = 'leased', attempt_count = $2, lease_owner = $3,
               lease_token = $4, lease_expires_at = $5, updated_at = now()
           WHERE id = $1"#,
    )
    .bind(job_id)
    .bind(attempt_number)
    .bind(worker_id)
    .bind(lease_token)
    .bind(lease_expires_at)
    .execute(&mut *transaction)
    .await?;
    let provider_snapshot = serde_json::json!({
        "kind": &provider_kind,
        "protocol": &provider_protocol,
        "base_url": &base_url,
        "model": &model,
        "credential_env_var": &credential_env_var,
    });
    sqlx::query(
        r#"INSERT INTO content_generation_attempts
           (id, job_id, attempt_number, provider_snapshot, request_hash, status)
           VALUES ($1,$2,$3,$4,$5,'running')"#,
    )
    .bind(attempt_id)
    .bind(job_id)
    .bind(attempt_number)
    .bind(&provider_snapshot)
    .bind(request_hash)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(GenerationLease {
        job_id,
        attempt_id,
        attempt_number,
        lease_token,
        provider_kind,
        provider_protocol,
        base_url,
        model,
        credential_env_var,
        request_spec,
        reserved_cost_microunits,
    }))
}

fn payload_hash(payload: &Value) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(payload).unwrap_or_default()).to_vec()
}

pub async fn complete_generation_job(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
    payload: Value,
    actual_cost_microunits: i64,
) -> Result<(), CompleteGenerationError> {
    let mut transaction = pool.begin().await?;
    let lease: Option<(i64, i16)> = sqlx::query_as(
        r#"SELECT reserved_cost_microunits, attempt_count FROM content_generation_jobs
           WHERE id = $1 AND lease_token = $2 AND status = 'leased'
             AND lease_expires_at > now() AND NOT settled FOR UPDATE"#,
    )
    .bind(job_id)
    .bind(lease_token)
    .fetch_optional(&mut *transaction)
    .await?;
    let (reserved, attempt_number) = lease.ok_or(CompleteGenerationError::LeaseLost)?;
    if actual_cost_microunits < 0 || actual_cost_microunits > reserved {
        return Err(CompleteGenerationError::CostExceeded);
    }
    let hash = payload_hash(&payload);
    let attempt = sqlx::query(
        "UPDATE content_generation_attempts SET status = 'completed', response_hash = $3, completed_at = now() WHERE id = $1 AND job_id = $2 AND status = 'running'",
    ).bind(attempt_id).bind(job_id).bind(&hash).execute(&mut *transaction).await?;
    if attempt.rows_affected() != 1 {
        return Err(CompleteGenerationError::LeaseLost);
    }
    sqlx::query(
        "INSERT INTO content_artifacts (job_id, attempt_id, artifact_kind, payload, content_hash) VALUES ($1,$2,'candidate',$3,$4)",
    ).bind(job_id).bind(attempt_id).bind(&payload).bind(&hash).execute(&mut *transaction).await?;
    sqlx::query(
        r#"UPDATE content_generation_jobs SET status = 'completed', settled = true,
           reserved_cost_microunits = 0, lease_owner = NULL, lease_token = NULL,
           lease_expires_at = NULL, updated_at = now() WHERE id = $1"#,
    )
    .bind(job_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1,
           spent_microunits = spent_microunits + $2, updated_at = now() WHERE budget_key = 'global'"#,
    ).bind(reserved).bind(actual_cost_microunits).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO audit_events (action, target_type, target_id, metadata) VALUES ('content_job.completed', 'content_generation_job', $1, jsonb_build_object('attempt', $2::smallint, 'cost_microunits', $3::bigint))")
        .bind(job_id.to_string()).bind(attempt_number).bind(actual_cost_microunits).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn fail_generation_job(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
    error_code: &str,
    receipt: Value,
    retryable: bool,
) -> Result<(), CompleteGenerationError> {
    let mut transaction = pool.begin().await?;
    let lease: Option<(i16, i16, i64)> = sqlx::query_as(
        r#"SELECT attempt_count, max_attempts, reserved_cost_microunits
           FROM content_generation_jobs WHERE id = $1 AND lease_token = $2
             AND status = 'leased' AND lease_expires_at > now() AND NOT settled FOR UPDATE"#,
    )
    .bind(job_id)
    .bind(lease_token)
    .fetch_optional(&mut *transaction)
    .await?;
    let (attempt_count, max_attempts, reserved) =
        lease.ok_or(CompleteGenerationError::LeaseLost)?;
    let will_retry = retryable && attempt_count < max_attempts;
    let hash = payload_hash(&receipt);
    let attempt = sqlx::query(
        "UPDATE content_generation_attempts SET status = $3, response_hash = $4, error_code = $5, completed_at = now() WHERE id = $1 AND job_id = $2 AND status = 'running'",
    ).bind(attempt_id).bind(job_id).bind(if will_retry { "retryable_failure" } else { "failed" })
        .bind(&hash).bind(error_code).execute(&mut *transaction).await?;
    if attempt.rows_affected() != 1 {
        return Err(CompleteGenerationError::LeaseLost);
    }
    sqlx::query(
        "INSERT INTO content_artifacts (job_id, attempt_id, artifact_kind, payload, content_hash) VALUES ($1,$2,'error_receipt',$3,$4)",
    ).bind(job_id).bind(attempt_id).bind(&receipt).bind(&hash).execute(&mut *transaction).await?;
    if will_retry {
        let backoff_seconds = 1_i64 << u32::from(attempt_count.saturating_sub(1).min(6) as u16);
        sqlx::query(
            r#"UPDATE content_generation_jobs SET status = 'queued', lease_owner = NULL,
               lease_token = NULL, lease_expires_at = NULL,
               available_at = now() + make_interval(secs => $3),
               last_error_code = $2, updated_at = now() WHERE id = $1"#,
        )
        .bind(job_id)
        .bind(error_code)
        .bind(backoff_seconds)
        .execute(&mut *transaction)
        .await?;
    } else {
        sqlx::query(
            r#"UPDATE content_generation_jobs SET status = 'failed', settled = true,
               reserved_cost_microunits = 0, lease_owner = NULL, lease_token = NULL,
               lease_expires_at = NULL, last_error_code = $2, updated_at = now() WHERE id = $1"#,
        )
        .bind(job_id)
        .bind(error_code)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, updated_at = now() WHERE budget_key = 'global'")
            .bind(reserved).execute(&mut *transaction).await?;
    }
    sqlx::query(
        "INSERT INTO audit_events (action, target_type, target_id, metadata) VALUES ($1, 'content_generation_job', $2, jsonb_build_object('attempt', $3::smallint, 'error_code', $4::text))",
    )
    .bind(if will_retry {
        "content_job.retry_scheduled"
    } else {
        "content_job.failed"
    })
    .bind(job_id.to_string())
    .bind(attempt_count)
    .bind(error_code)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}
