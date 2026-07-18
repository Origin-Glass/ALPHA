use std::net::IpAddr;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Host, Url};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

pub const CONTENT_PROMPT_TEMPLATE_VERSION: &str = "content-v1";
pub const CONTENT_PROMPT_TEMPLATE: &str =
    "다음 명세에 맞는 한국어 교육 콘텐츠를 JSON으로 생성하세요: ";

type ProviderRow = (
    Uuid,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<String>,
    bool,
    String,
    Option<time::OffsetDateTime>,
    bool,
    bool,
    Option<Uuid>,
);
type JobProviderPricing = (
    bool,
    String,
    Option<String>,
    i64,
    Option<Uuid>,
    Option<bool>,
    Option<i64>,
    Option<String>,
);
type CloneSource = (
    Uuid,
    Option<Uuid>,
    String,
    Value,
    Vec<u8>,
    String,
    i64,
    i64,
    bool,
    Option<String>,
    String,
    Option<bool>,
    Option<String>,
);
type JobRow = (
    Uuid,
    String,
    String,
    String,
    i16,
    Option<String>,
    time::OffsetDateTime,
    bool,
);
type LeaseCandidate = (
    Uuid,
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
    i64,
    String,
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
    #[serde(default)]
    supports_stream: bool,
    #[serde(default)]
    supports_tools: bool,
    #[serde(default)]
    fallback_provider_id: Option<Uuid>,
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
        || request.fallback_provider_id.is_some()
        || request.supports_stream
        || request.supports_tools
    {
        return Err(ContentFactoryError::InvalidInput(
            "제공자 설정을 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO content_provider_configs
           (name, kind, protocol, base_url, model, cost_per_generation_microunits,
            credential_env_var, enabled, supports_stream, supports_tools, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id"#,
    )
    .bind(&request.name)
    .bind(&request.kind)
    .bind(&request.protocol)
    .bind(&request.base_url)
    .bind(request.model.trim())
    .bind(request.cost_per_generation_microunits)
    .bind(&request.credential_env_var)
    .bind(request.enabled)
    .bind(request.supports_stream)
    .bind(request.supports_tools)
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
        r#"SELECT provider.id, provider.name, provider.kind, provider.protocol, provider.model,
                      provider.cost_per_generation_microunits,
                      GREATEST(provider.cost_per_generation_microunits, COALESCE(fallback.cost_per_generation_microunits, 0)),
                      provider.credential_env_var, provider.enabled,
                      CASE WHEN provider.health_status = 'unhealthy' AND provider.last_health_at <= now() - interval '10 minutes'
                           THEN 'unverified' ELSE provider.health_status END,
                      provider.last_health_at, provider.supports_stream, provider.supports_tools,
                      provider.fallback_provider_id
               FROM content_provider_configs provider
               LEFT JOIN content_provider_configs fallback ON fallback.id = provider.fallback_provider_id
               ORDER BY provider.name"#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(serde_json::json!({
        "providers": providers.into_iter().map(|(id, name, kind, protocol, model, cost, liability_cost, credential_env_var, enabled, health_status, last_health_at, supports_stream, supports_tools, fallback_provider_id)| serde_json::json!({
            "id": id,
            "name": name,
            "kind": kind,
            "protocol": protocol,
            "model": model,
            "cost_per_generation_microunits": cost,
            "liability_cost_per_generation_microunits": liability_cost,
            "credential_available": credential_env_var.as_deref().is_none_or(|name| state.settings().has_content_ai_credential(name)),
            "enabled": enabled,
            "health_status": health_status,
            "last_health_at": last_health_at,
            "supports_stream": supports_stream,
            "supports_tools": supports_tools,
            "fallback_provider_id": fallback_provider_id,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderRequest {
    enabled: bool,
    supports_stream: bool,
    supports_tools: bool,
    fallback_provider_id: Option<Uuid>,
}

pub async fn update_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<UpdateProviderRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(&state, &headers, "provider.configure").await?;
    if request.fallback_provider_id == Some(provider_id) {
        return Err(ContentFactoryError::InvalidInput(
            "제공자는 자기 자신을 대체 제공자로 지정할 수 없습니다",
        ));
    }
    if request.supports_stream || request.supports_tools {
        return Err(ContentFactoryError::InvalidInput(
            "현재 콘텐츠 worker는 스트리밍·도구 호출을 지원하지 않습니다",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(784512903)")
        .execute(&mut *transaction)
        .await?;
    if let Some(fallback_id) = request.fallback_provider_id {
        let compatible: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                SELECT 1 FROM content_provider_configs primary_provider
                JOIN content_provider_configs fallback ON fallback.id = $2
                WHERE primary_provider.id = $1 AND fallback.enabled
                  AND fallback.archived_at IS NULL AND fallback.kind = primary_provider.kind
                  AND fallback.id <> primary_provider.id
            )"#,
        )
        .bind(provider_id)
        .bind(fallback_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !compatible {
            return Err(ContentFactoryError::InvalidInput(
                "동일 종류의 활성 대체 제공자를 선택하고 순환 구성을 제거해 주세요",
            ));
        }
        let creates_cycle: bool = sqlx::query_scalar(
            r#"WITH RECURSIVE chain(id, path, is_cycle) AS (
                   SELECT $2::uuid, ARRAY[$1::uuid, $2::uuid], $2::uuid = $1::uuid
                   UNION ALL
                   SELECT provider.fallback_provider_id,
                          chain.path || provider.fallback_provider_id,
                          provider.fallback_provider_id = ANY(chain.path)
                   FROM chain
                   JOIN content_provider_configs provider ON provider.id = chain.id
                   WHERE provider.fallback_provider_id IS NOT NULL AND NOT chain.is_cycle
               )
               SELECT EXISTS(SELECT 1 FROM chain WHERE is_cycle)"#,
        )
        .bind(provider_id)
        .bind(fallback_id)
        .fetch_one(&mut *transaction)
        .await?;
        if creates_cycle {
            return Err(ContentFactoryError::InvalidInput(
                "대체 제공자 순환 구성을 제거해 주세요",
            ));
        }
    }
    let updated = sqlx::query(
        r#"UPDATE content_provider_configs SET enabled = $2, supports_stream = $3,
           supports_tools = $4, fallback_provider_id = $5, updated_at = now()
           WHERE id = $1 AND archived_at IS NULL"#,
    )
    .bind(provider_id)
    .bind(request.enabled)
    .bind(request.supports_stream)
    .bind(request.supports_tools)
    .bind(request.fallback_provider_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ContentFactoryError::NotFound);
    }
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_provider.updated', 'content_provider', $2, jsonb_build_object('enabled', $3::boolean, 'supports_stream', $4::boolean, 'supports_tools', $5::boolean, 'fallback_provider_id', $6::uuid))")
        .bind(user_id).bind(provider_id.to_string()).bind(request.enabled).bind(request.supports_stream)
        .bind(request.supports_tools).bind(request.fallback_provider_id).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"id": provider_id, "enabled": request.enabled}),
    ))
}

pub async fn validate_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(&state, &headers, "provider.configure").await?;
    let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT kind, base_url, model, credential_env_var FROM content_provider_configs WHERE id = $1 AND archived_at IS NULL",
    ).bind(provider_id).fetch_optional(state.pool()).await?;
    let (kind, base_url, model, credential_env_var) = row.ok_or(ContentFactoryError::NotFound)?;
    validate_provider_url(&kind, &base_url)?;
    if model.trim().is_empty()
        || credential_env_var
            .as_deref()
            .is_some_and(|name| !state.settings().has_content_ai_credential(name))
    {
        return Err(ContentFactoryError::Conflict(
            "모델 또는 자격 증명 설정을 확인해 주세요",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    sqlx::query("UPDATE content_provider_configs SET health_status = 'unverified', consecutive_failures = 0, last_health_at = NULL, updated_at = now() WHERE id = $1")
        .bind(provider_id).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_provider.validation_requested', 'content_provider', $2, jsonb_build_object('model', $3::text))")
        .bind(user_id).bind(provider_id.to_string()).bind(&model).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"id": provider_id, "configuration_valid": true, "execution_health": "unverified"}),
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateJobRequest {
    idempotency_key: Uuid,
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
    let spec = serde_json::json!({
        "provider_id": request.provider_id,
        "content_type": &request.content_type,
        "topic": request.topic.trim(),
        "target_language": &request.target_language,
        "generation_count": request.generation_count,
    });
    let request_hash = Sha256::digest(serde_json::to_vec(&spec).unwrap_or_default()).to_vec();
    let mut transaction = state.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "content:create:{user_id}:{}",
            request.idempotency_key
        ))
        .execute(&mut *transaction)
        .await?;
    let replay: Option<(Uuid, String, Vec<u8>)> = sqlx::query_as(
        "SELECT id, status, request_hash FROM content_generation_jobs WHERE created_by = $1 AND create_idempotency_key = $2",
    )
    .bind(user_id)
    .bind(request.idempotency_key)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some((id, status, prior_hash)) = replay {
        if prior_hash != request_hash {
            return Err(ContentFactoryError::Conflict(
                "같은 멱등키에 다른 생성 요청을 사용할 수 없습니다",
            ));
        }
        transaction.commit().await?;
        return Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({"id": id, "status": status, "idempotent_replay": true})),
        ));
    }
    let provider: Option<JobProviderPricing> = sqlx::query_as(
        r#"SELECT provider.enabled, provider.kind, provider.credential_env_var,
                  provider.cost_per_generation_microunits, provider.fallback_provider_id,
                  fallback.enabled, fallback.cost_per_generation_microunits, fallback.credential_env_var
           FROM content_provider_configs provider
           LEFT JOIN content_provider_configs fallback ON fallback.id = provider.fallback_provider_id
           WHERE provider.id = $1 AND provider.archived_at IS NULL"#,
    )
    .bind(request.provider_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (
        enabled,
        provider_kind,
        credential_env_var,
        unit_cost,
        fallback_id,
        fallback_enabled,
        fallback_cost,
        fallback_credential,
    ) = provider.ok_or(ContentFactoryError::NotFound)?;
    let liability_unit_cost = unit_cost.max(fallback_cost.unwrap_or(0));
    let attempt_cost_microunits = liability_unit_cost
        .checked_mul(i64::from(request.generation_count))
        .ok_or(ContentFactoryError::InvalidInput(
            "제공자 가격을 확인해 주세요",
        ))?;
    let estimated_cost_microunits =
        attempt_cost_microunits
            .checked_mul(3)
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
    let status = if !state.settings().content_ai_enabled
        || !enabled
        || (fallback_id.is_some() && fallback_enabled != Some(true))
    {
        "blocked_disabled"
    } else if credential_env_var
        .as_deref()
        .is_some_and(|name| !state.settings().has_content_ai_credential(name))
        || fallback_credential
            .as_deref()
            .is_some_and(|name| !state.settings().has_content_ai_credential(name))
    {
        "blocked_missing_credential"
    } else {
        "queued"
    };
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
           (created_by, provider_id, fallback_provider_id_snapshot, content_type, request_spec, request_hash, status,
            estimated_cost_microunits, attempt_cost_microunits, reserved_cost_microunits,
            create_idempotency_key)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id"#,
    )
    .bind(user_id)
    .bind(request.provider_id)
    .bind(fallback_id)
    .bind(&request.content_type)
    .bind(&spec)
    .bind(request_hash)
    .bind(status)
    .bind(estimated_cost_microunits)
    .bind(attempt_cost_microunits)
    .bind(reserved)
    .bind(request.idempotency_key)
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
                      job.last_error_code, job.created_at,
                      EXISTS(SELECT 1 FROM content_artifacts artifact WHERE artifact.job_id = job.id AND artifact.artifact_kind = 'candidate')
               FROM content_generation_jobs job
               JOIN content_provider_configs provider ON provider.id = job.provider_id
               WHERE job.created_by = $1 AND job.archived_at IS NULL
               ORDER BY job.created_at DESC LIMIT 50"#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(
        serde_json::json!({"jobs": jobs.into_iter().map(|(id, provider, content_type, status, attempts, error, created_at, has_artifact)| serde_json::json!({
        "id": id, "provider": provider, "content_type": content_type, "status": status,
        "attempts": attempts, "last_error_code": error, "created_at": created_at, "has_artifact": has_artifact,
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

pub async fn job_detail(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize_read(&state, &headers, "content.generate").await?;
    let job: Option<Value> = sqlx::query_scalar(
        r#"SELECT jsonb_build_object(
             'id', job.id, 'provider', provider.name, 'content_type', job.content_type,
             'request_spec', job.request_spec, 'status', job.status, 'attempts', job.attempt_count,
             'max_attempts', job.max_attempts, 'reserved_microunits', job.reserved_cost_microunits,
             'last_error_code', job.last_error_code, 'created_at', job.created_at,
             'updated_at', job.updated_at, 'archived_at', job.archived_at,
             'cloned_from_job_id', job.cloned_from_job_id)
           FROM content_generation_jobs job
           JOIN content_provider_configs provider ON provider.id = job.provider_id
           WHERE job.id = $1 AND job.created_by = $2"#,
    )
    .bind(job_id)
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?;
    let job = job.ok_or(ContentFactoryError::NotFound)?;
    let artifacts: Vec<Value> = sqlx::query_scalar(
        r#"SELECT jsonb_build_object('id', artifact.id, 'attempt_id', artifact.attempt_id,
                  'kind', artifact.artifact_kind, 'payload', artifact.payload,
                  'hash', encode(artifact.content_hash, 'hex'), 'created_at', artifact.created_at)
           FROM content_artifacts artifact
           WHERE artifact.job_id = $1 ORDER BY artifact.created_at"#,
    )
    .bind(job_id)
    .fetch_all(state.pool())
    .await?;
    let attempts: Vec<Value> = sqlx::query_scalar(
        r#"SELECT jsonb_build_object('id', id, 'attempt_number', attempt_number,
                  'provider_snapshot', provider_snapshot, 'request_hash', encode(request_hash, 'hex'),
                  'response_hash', CASE WHEN response_hash IS NULL THEN NULL ELSE encode(response_hash, 'hex') END,
                  'status', status, 'error_code', error_code, 'calls_started', calls_started,
                  'outputs_completed', outputs_completed, 'usage', usage,
                  'started_at', started_at, 'completed_at', completed_at)
           FROM content_generation_attempts WHERE job_id = $1 ORDER BY attempt_number"#,
    ).bind(job_id).fetch_all(state.pool()).await?;
    let audit: Vec<Value> = sqlx::query_scalar(
        r#"SELECT jsonb_build_object('action', action, 'metadata', metadata, 'occurred_at', occurred_at)
           FROM audit_events WHERE target_type = 'content_generation_job' AND target_id = $1 ORDER BY occurred_at"#,
    ).bind(job_id.to_string()).fetch_all(state.pool()).await?;
    Ok(Json(serde_json::json!({
        "job": job, "attempts": attempts, "artifacts": artifacts, "audit": audit
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobActionRequest {
    idempotency_key: Uuid,
}

pub async fn cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<JobActionRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(&state, &headers, "content.generate").await?;
    let mut transaction = state.pool().begin().await?;
    let row: Option<(String, i64, Option<Uuid>, i16)> = sqlx::query_as(
        "SELECT status, reserved_cost_microunits, cancel_idempotency_key, attempt_count FROM content_generation_jobs WHERE id = $1 AND created_by = $2 FOR UPDATE",
    )
    .bind(job_id).bind(user_id).fetch_optional(&mut *transaction).await?;
    let (status, reserved, prior_key, attempt_count) = row.ok_or(ContentFactoryError::NotFound)?;
    if prior_key == Some(request.idempotency_key) {
        transaction.commit().await?;
        return Ok(Json(
            serde_json::json!({"id": job_id, "status": "cancelled", "idempotent_replay": true}),
        ));
    }
    if prior_key.is_some()
        || !matches!(
            status.as_str(),
            "queued" | "leased" | "blocked_disabled" | "blocked_missing_credential"
        )
    {
        return Err(ContentFactoryError::Conflict(
            "현재 상태에서는 작업을 취소할 수 없습니다",
        ));
    }
    if status == "leased" {
        sqlx::query("UPDATE content_generation_attempts SET status = 'cancelled', error_code = 'owner_cancelled', completed_at = now() WHERE job_id = $1 AND attempt_number = $2 AND status = 'running'")
            .bind(job_id).bind(attempt_count).execute(&mut *transaction).await?;
    }
    sqlx::query("UPDATE content_generation_jobs SET status = 'cancelled', settled = true, reserved_cost_microunits = 0, lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, cancel_idempotency_key = $2, updated_at = now() WHERE id = $1")
        .bind(job_id).bind(request.idempotency_key).execute(&mut *transaction).await?;
    let charged: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM(a.calls_started::bigint *
             (j.attempt_cost_microunits / (j.request_spec->>'generation_count')::bigint)), 0)::bigint
           FROM content_generation_attempts a JOIN content_generation_jobs j ON j.id = a.job_id
           WHERE a.job_id = $1"#,
    )
    .bind(job_id)
    .fetch_one(&mut *transaction)
    .await?;
    if reserved > 0 {
        sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, spent_microunits = spent_microunits + $2, updated_at = now() WHERE budget_key = 'global'")
            .bind(reserved).bind(charged).execute(&mut *transaction).await?;
    }
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_job.cancelled', 'content_generation_job', $2, jsonb_build_object('idempotency_key', $3::uuid, 'charged_microunits', $4::bigint))")
        .bind(user_id).bind(job_id.to_string()).bind(request.idempotency_key).bind(charged).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"id": job_id, "status": "cancelled"}),
    ))
}

async fn clone_job_inner(
    state: &AppState,
    headers: &HeaderMap,
    job_id: Uuid,
    request: JobActionRequest,
    retry_only: bool,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(state, headers, "content.generate").await?;
    let mut transaction = state.pool().begin().await?;
    let action = if retry_only { "retry" } else { "clone" };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "content:{action}:{user_id}:{job_id}:{}",
            request.idempotency_key
        ))
        .execute(&mut *transaction)
        .await?;
    let replay: Option<(Uuid, String)> = sqlx::query_as(
        r#"SELECT id, status FROM content_generation_jobs
           WHERE created_by = $1 AND cloned_from_job_id = $2
             AND (($4 AND retry_idempotency_key = $3) OR (NOT $4 AND clone_idempotency_key = $3))"#,
    )
    .bind(user_id)
    .bind(job_id)
    .bind(request.idempotency_key)
    .bind(retry_only)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some((id, status)) = replay {
        transaction.commit().await?;
        return Ok(Json(
            serde_json::json!({"id": id, "status": status, "idempotent_replay": true}),
        ));
    }
    let source: Option<CloneSource> = sqlx::query_as(
        r#"SELECT job.provider_id, job.fallback_provider_id_snapshot, job.content_type, job.request_spec, job.request_hash, job.status,
                  job.estimated_cost_microunits, job.attempt_cost_microunits,
                  provider.enabled, provider.credential_env_var, provider.kind,
                  fallback.enabled, fallback.credential_env_var
           FROM content_generation_jobs job JOIN content_provider_configs provider ON provider.id = job.provider_id
           LEFT JOIN content_provider_configs fallback ON fallback.id = job.fallback_provider_id_snapshot
           WHERE job.id = $1 AND job.created_by = $2 FOR UPDATE OF job"#,
    ).bind(job_id).bind(user_id).fetch_optional(&mut *transaction).await?;
    let (
        provider_id,
        fallback_id,
        content_type,
        spec,
        request_hash,
        source_status,
        estimated,
        attempt_cost,
        enabled,
        credential_env,
        provider_kind,
        fallback_enabled,
        fallback_credential,
    ) = source.ok_or(ContentFactoryError::NotFound)?;
    let provider_capability = if provider_kind == "local" {
        "provider.use.local"
    } else {
        "provider.use.frontier"
    };
    if !crate::governance::has_capability(state.pool(), user_id, provider_capability).await? {
        return Err(ContentFactoryError::Forbidden);
    }
    if retry_only && source_status != "failed" {
        return Err(ContentFactoryError::Conflict(
            "실패한 작업만 수동 재시도할 수 있습니다",
        ));
    }
    if !retry_only && source_status == "leased" {
        return Err(ContentFactoryError::Conflict(
            "실행 중인 작업은 복제할 수 없습니다",
        ));
    }
    let status = if !state.settings().content_ai_enabled
        || !enabled
        || (fallback_id.is_some() && fallback_enabled != Some(true))
    {
        "blocked_disabled"
    } else if credential_env
        .as_deref()
        .is_some_and(|name| !state.settings().has_content_ai_credential(name))
        || fallback_credential
            .as_deref()
            .is_some_and(|name| !state.settings().has_content_ai_credential(name))
    {
        "blocked_missing_credential"
    } else {
        "queued"
    };
    let reserved = if status == "queued" {
        let updated = sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits + $1, updated_at = now() WHERE budget_key = 'global' AND reserved_microunits + spent_microunits + $1 <= limit_microunits")
            .bind(estimated).execute(&mut *transaction).await?;
        if updated.rows_affected() != 1 {
            return Err(ContentFactoryError::Conflict(
                "콘텐츠 생성 예산이 부족합니다",
            ));
        }
        estimated
    } else {
        0
    };
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO content_generation_jobs
           (created_by, provider_id, fallback_provider_id_snapshot, content_type, request_spec, request_hash, status,
            estimated_cost_microunits, attempt_cost_microunits, reserved_cost_microunits,
            cloned_from_job_id, clone_idempotency_key, retry_idempotency_key)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id"#,
    )
    .bind(user_id)
    .bind(provider_id)
    .bind(fallback_id)
    .bind(content_type)
    .bind(spec)
    .bind(request_hash)
    .bind(status)
    .bind(estimated)
    .bind(attempt_cost)
    .bind(reserved)
    .bind(job_id)
    .bind((!retry_only).then_some(request.idempotency_key))
    .bind(retry_only.then_some(request.idempotency_key))
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'content_generation_job', $3, jsonb_build_object('source_job_id', $4::uuid, 'idempotency_key', $5::uuid))")
        .bind(user_id).bind(if retry_only { "content_job.manual_retry" } else { "content_job.cloned" })
        .bind(id.to_string()).bind(job_id).bind(request.idempotency_key).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"id": id, "status": status})))
}

pub async fn clone_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<JobActionRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    clone_job_inner(&state, &headers, job_id, request, false).await
}

pub async fn retry_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<JobActionRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    clone_job_inner(&state, &headers, job_id, request, true).await
}

pub async fn archive_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<JobActionRequest>,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize(&state, &headers, "content.generate").await?;
    let mut transaction = state.pool().begin().await?;
    let row: Option<(String, Option<Uuid>)> = sqlx::query_as("SELECT status, archive_idempotency_key FROM content_generation_jobs WHERE id = $1 AND created_by = $2 FOR UPDATE")
        .bind(job_id).bind(user_id).fetch_optional(&mut *transaction).await?;
    let (status, prior_key) = row.ok_or(ContentFactoryError::NotFound)?;
    if prior_key == Some(request.idempotency_key) {
        transaction.commit().await?;
        return Ok(Json(
            serde_json::json!({"id": job_id, "archived": true, "idempotent_replay": true}),
        ));
    }
    if prior_key.is_some() || !matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(ContentFactoryError::Conflict(
            "종료된 작업만 보관할 수 있습니다",
        ));
    }
    sqlx::query("UPDATE content_generation_jobs SET archived_at = now(), archive_idempotency_key = $2, updated_at = now() WHERE id = $1")
        .bind(job_id).bind(request.idempotency_key).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'content_job.archived', 'content_generation_job', $2, jsonb_build_object('idempotency_key', $3::uuid))")
        .bind(user_id).bind(job_id.to_string()).bind(request.idempotency_key).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"id": job_id, "archived": true})))
}

#[derive(Deserialize)]
pub struct CompareJobsQuery {
    left: Uuid,
    right: Uuid,
}

pub async fn compare_jobs(
    State(state): State<AppState>,
    Query(query): Query<CompareJobsQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ContentFactoryError> {
    let user_id = authorize_read(&state, &headers, "content.generate").await?;
    let rows: Vec<(Uuid, Value)> = sqlx::query_as(
        r#"SELECT job.id, artifact.payload FROM content_generation_jobs job
           JOIN LATERAL (SELECT payload FROM content_artifacts WHERE job_id = job.id AND artifact_kind = 'candidate' ORDER BY created_at DESC LIMIT 1) artifact ON true
           WHERE job.created_by = $1 AND job.id = ANY($2)"#,
    ).bind(user_id).bind(vec![query.left, query.right]).fetch_all(state.pool()).await?;
    if rows.len() != 2 {
        return Err(ContentFactoryError::NotFound);
    }
    Ok(Json(
        serde_json::json!({"jobs": rows.into_iter().map(|(id, artifact)| serde_json::json!({"id": id, "artifact": artifact})).collect::<Vec<_>>()}),
    ))
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
    pub provider_id: Uuid,
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
    pub attempt_cost_microunits: i64,
    pub content_type: String,
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
    let expired_leases: Vec<(Uuid, i16, i16, i64, i64)> = sqlx::query_as(
        r#"SELECT id, attempt_count, max_attempts, reserved_cost_microunits, attempt_cost_microunits
           FROM content_generation_jobs
           WHERE status = 'leased' AND lease_expires_at <= now() AND NOT settled
           FOR UPDATE"#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    for (job_id, attempt, max_attempts, reserved, _) in &expired_leases {
        let outcome = if attempt >= max_attempts {
            "failed"
        } else {
            "requeued"
        };
        let receipt = serde_json::json!({"error": "작업 lease 만료", "attempt": attempt});
        let hash = payload_hash(&receipt);
        let attempt_id: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE content_generation_attempts
               SET status = $3, error_code = 'lease_expired', response_hash = $4, completed_at = now()
               WHERE job_id = $1 AND attempt_number = $2 AND status = 'running'
               RETURNING id"#,
        )
        .bind(*job_id)
        .bind(*attempt)
        .bind(if outcome == "failed" { "failed" } else { "retryable_failure" })
        .bind(&hash)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(attempt_id) = attempt_id {
            sqlx::query("INSERT INTO content_artifacts (job_id, attempt_id, artifact_kind, payload, content_hash) VALUES ($1,$2,'error_receipt',$3,$4)")
                .bind(*job_id).bind(attempt_id).bind(&receipt).bind(&hash)
                .execute(&mut *transaction).await?;
        }
        if outcome == "failed" {
            let charged: i64 = sqlx::query_scalar(
                r#"SELECT COALESCE(SUM(a.calls_started::bigint *
                     (j.attempt_cost_microunits / (j.request_spec->>'generation_count')::bigint)), 0)::bigint
                   FROM content_generation_attempts a
                   JOIN content_generation_jobs j ON j.id = a.job_id WHERE a.job_id = $1"#,
            ).bind(*job_id).fetch_one(&mut *transaction).await?;
            sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, spent_microunits = spent_microunits + $2, updated_at = now() WHERE budget_key = 'global'")
                .bind(*reserved).bind(charged).execute(&mut *transaction).await?;
            sqlx::query(r#"UPDATE content_generation_jobs SET status = 'failed', settled = true,
                reserved_cost_microunits = 0, lease_owner = NULL, lease_token = NULL,
                lease_expires_at = NULL, last_error_code = 'lease_attempts_exhausted', updated_at = now()
                WHERE id = $1"#).bind(*job_id).execute(&mut *transaction).await?;
        } else {
            sqlx::query(r#"UPDATE content_generation_jobs SET status = 'queued', lease_owner = NULL,
                lease_token = NULL, lease_expires_at = NULL,
                available_at = now() + make_interval(secs => (1 << LEAST(GREATEST(attempt_count - 1, 0), 6))),
                last_error_code = 'lease_expired', updated_at = now() WHERE id = $1"#)
                .bind(*job_id).execute(&mut *transaction).await?;
        }
        sqlx::query(
            "INSERT INTO audit_events (action, target_type, target_id, metadata) VALUES ('content_job.lease_expired', 'content_generation_job', $1, jsonb_build_object('attempt', $2::smallint, 'outcome', $3::text))",
        )
        .bind(job_id.to_string())
        .bind(*attempt)
        .bind(outcome)
        .execute(&mut *transaction)
        .await?;
    }

    let disabled: Vec<(Uuid, i64)> = sqlx::query_as(
        r#"SELECT job.id, job.reserved_cost_microunits FROM content_generation_jobs job
           JOIN content_provider_configs provider ON provider.id = job.provider_id
           WHERE job.status = 'queued' AND (NOT provider.enabled OR provider.archived_at IS NOT NULL)
           FOR UPDATE OF job"#,
    ).fetch_all(&mut *transaction).await?;
    for (job_id, reserved) in disabled {
        sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, updated_at = now() WHERE budget_key = 'global'")
            .bind(reserved).execute(&mut *transaction).await?;
        sqlx::query("UPDATE content_generation_jobs SET status = 'blocked_disabled', settled = true, reserved_cost_microunits = 0, last_error_code = 'provider_disabled_before_execution', updated_at = now() WHERE id = $1")
            .bind(job_id).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO audit_events (action, target_type, target_id, metadata) VALUES ('content_job.provider_kill_switch', 'content_generation_job', $1, jsonb_build_object('released_microunits', $2::bigint))")
            .bind(job_id.to_string()).bind(reserved).execute(&mut *transaction).await?;
    }

    let candidate: Option<LeaseCandidate> = sqlx::query_as(
        r#"SELECT job.id, COALESCE(fallback.id, provider.id), job.attempt_count,
                      COALESCE(fallback.kind, provider.kind), COALESCE(fallback.protocol, provider.protocol),
                      COALESCE(fallback.base_url, provider.base_url), COALESCE(fallback.model, provider.model),
                      COALESCE(fallback.credential_env_var, provider.credential_env_var), job.request_spec,
                      job.request_hash, job.reserved_cost_microunits, job.attempt_cost_microunits,
                      job.content_type
               FROM content_generation_jobs job
               JOIN content_provider_configs provider ON provider.id = job.provider_id
               LEFT JOIN content_provider_configs fallback
                 ON fallback.id = job.fallback_provider_id_snapshot AND fallback.enabled
                AND fallback.archived_at IS NULL AND provider.health_status = 'unhealthy'
                AND provider.consecutive_failures >= 3
                AND provider.last_health_at > now() - interval '10 minutes'
                AND NOT (fallback.health_status = 'unhealthy'
                         AND fallback.last_health_at > now() - interval '10 minutes')
               WHERE job.status = 'queued' AND job.available_at <= now()
                 AND provider.enabled AND provider.archived_at IS NULL
               ORDER BY job.available_at, job.created_at
               FOR UPDATE OF job SKIP LOCKED LIMIT 1"#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((
        job_id,
        provider_id,
        attempt_count,
        provider_kind,
        provider_protocol,
        base_url,
        model,
        credential_env_var,
        request_spec,
        request_hash,
        reserved_cost_microunits,
        attempt_cost_microunits,
        content_type,
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
    let prompt_template_hash = Sha256::digest(CONTENT_PROMPT_TEMPLATE.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let provider_snapshot = serde_json::json!({
        "kind": &provider_kind,
        "provider_id": provider_id,
        "protocol": &provider_protocol,
        "base_url": &base_url,
        "model": &model,
        "credential_env_var": &credential_env_var,
        "prompt_template_version": CONTENT_PROMPT_TEMPLATE_VERSION,
        "prompt_template_hash": prompt_template_hash,
        "seed_policy": "sha256(job_id:output_index)",
        "provider_idempotency_key_policy": "job_id:output_index",
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
        provider_id,
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
        attempt_cost_microunits,
        content_type,
    }))
}

fn payload_hash(payload: &Value) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(payload).unwrap_or_default()).to_vec()
}

pub async fn renew_generation_lease(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
    lease_duration: std::time::Duration,
) -> Result<bool, sqlx::Error> {
    let seconds = i64::try_from(lease_duration.as_secs())
        .unwrap_or(300)
        .clamp(1, 300);
    let updated = sqlx::query(
        r#"UPDATE content_generation_jobs job SET lease_expires_at = now() + make_interval(secs => $4), updated_at = now()
           FROM content_generation_attempts attempt
           WHERE job.id = $1 AND job.lease_token = $2 AND job.status = 'leased' AND NOT job.settled
             AND job.lease_expires_at > now() AND attempt.id = $3 AND attempt.job_id = job.id
             AND attempt.status = 'running'"#,
    ).bind(job_id).bind(lease_token).bind(attempt_id).bind(seconds).execute(pool).await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn begin_provider_call(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    provider_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let fenced: Option<bool> = sqlx::query_scalar(
        r#"SELECT original.enabled AND selected.enabled FROM content_generation_jobs job
           JOIN content_provider_configs original ON original.id = job.provider_id
           JOIN content_provider_configs selected ON selected.id = $2
           WHERE job.id = $1 AND job.lease_token = $3 AND job.status = 'leased'
             AND job.lease_expires_at > now() AND NOT job.settled
             AND original.enabled AND original.archived_at IS NULL
             AND selected.enabled AND selected.archived_at IS NULL FOR UPDATE OF job"#,
    )
    .bind(job_id)
    .bind(provider_id)
    .bind(lease_token)
    .fetch_optional(&mut *transaction)
    .await?;
    if fenced != Some(true) {
        transaction.commit().await?;
        return Ok(false);
    }
    let updated = sqlx::query(
        r#"UPDATE content_generation_attempts SET calls_started = calls_started + 1
           WHERE id = $1 AND job_id = $2 AND status = 'running' AND calls_started < 20"#,
    )
    .bind(attempt_id)
    .bind(job_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn record_provider_output(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
    usage: &Value,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        r#"UPDATE content_generation_attempts attempt
           SET outputs_completed = outputs_completed + 1, usage = usage || $4::jsonb
           FROM content_generation_jobs job
           WHERE attempt.id = $2 AND attempt.job_id = $1 AND attempt.status = 'running'
             AND job.id = attempt.job_id AND job.lease_token = $3 AND job.status = 'leased'
             AND job.lease_expires_at > now()"#,
    )
    .bind(job_id)
    .bind(attempt_id)
    .bind(lease_token)
    .bind(usage)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn complete_generation_job(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempt_id: Uuid,
    lease_token: Uuid,
    payload: Value,
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
    let actual_cost_microunits: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM(a.calls_started::bigint *
             (j.attempt_cost_microunits / (j.request_spec->>'generation_count')::bigint)), 0)::bigint
           FROM content_generation_attempts a JOIN content_generation_jobs j ON j.id = a.job_id
           WHERE a.job_id = $1"#,
    )
    .bind(job_id)
    .fetch_one(&mut *transaction)
    .await?;
    if actual_cost_microunits > reserved {
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
        let charged: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(SUM(a.calls_started::bigint *
                 (j.attempt_cost_microunits / (j.request_spec->>'generation_count')::bigint)), 0)::bigint
               FROM content_generation_attempts a JOIN content_generation_jobs j ON j.id = a.job_id
               WHERE a.job_id = $1"#,
        )
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE content_budgets SET reserved_microunits = reserved_microunits - $1, spent_microunits = spent_microunits + $2, updated_at = now() WHERE budget_key = 'global'")
            .bind(reserved).bind(charged).execute(&mut *transaction).await?;
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
