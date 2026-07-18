use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;

use crate::http::AppState;

const SOLVED_AC_V3_ENDPOINT: &str = "https://solved.ac/api/v3/problem/show";

pub trait ProblemMetadataProvider {
    fn provider(&self) -> &'static str;
    fn revision(&self) -> &'static str;
    fn normalize(&self, payload: &[u8]) -> Result<NormalizedMetadata, MetadataError>;
}

pub struct SolvedAcV3;

#[derive(Debug, Deserialize)]
struct SolvedAcPayload {
    #[serde(rename = "problemId")]
    _problem_id: i64,
    level: i16,
    #[serde(default)]
    tags: Vec<SolvedAcTag>,
}

#[derive(Debug, Deserialize)]
struct SolvedAcTag {
    key: String,
    #[serde(rename = "displayNames", default)]
    display_names: Vec<SolvedAcDisplayName>,
}

#[derive(Debug, Deserialize)]
struct SolvedAcDisplayName {
    language: String,
    name: String,
    #[serde(rename = "short")]
    _short: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NormalizedTag {
    pub tag: String,
    pub label: String,
}

#[derive(Debug)]
pub struct NormalizedMetadata {
    raw_level: i16,
    normalized_tier: String,
    tags: Vec<NormalizedTag>,
    response_hash: String,
    payload: serde_json::Value,
}

#[derive(Debug)]
pub enum MetadataError {
    InvalidExternalId,
    InvalidEndpoint,
    ProviderStatus(StatusCode),
    ProviderRequest,
    InvalidProviderResponse,
    Database(sqlx::Error),
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidExternalId => formatter.write_str("외부 문제 번호가 올바르지 않습니다"),
            Self::InvalidEndpoint => {
                formatter.write_str("메타데이터 제공자 주소가 올바르지 않습니다")
            }
            Self::ProviderStatus(status) => write!(formatter, "메타데이터 제공자 HTTP {status}"),
            Self::ProviderRequest => formatter.write_str("메타데이터 제공자에 연결하지 못했습니다"),
            Self::InvalidProviderResponse => {
                formatter.write_str("메타데이터 제공자 응답 형식이 올바르지 않습니다")
            }
            Self::Database(_) => formatter.write_str("메타데이터 저장에 실패했습니다"),
        }
    }
}

impl std::error::Error for MetadataError {}

impl From<sqlx::Error> for MetadataError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl ProblemMetadataProvider for SolvedAcV3 {
    fn provider(&self) -> &'static str {
        "solved_ac"
    }

    fn revision(&self) -> &'static str {
        "api-v3"
    }

    fn normalize(&self, payload: &[u8]) -> Result<NormalizedMetadata, MetadataError> {
        let decoded: SolvedAcPayload =
            serde_json::from_slice(payload).map_err(|_| MetadataError::InvalidProviderResponse)?;
        if decoded.level < 0 {
            return Err(MetadataError::InvalidProviderResponse);
        }
        let mut tags = BTreeMap::new();
        for tag in decoded.tags {
            if tag.key.is_empty()
                || !tag
                    .key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(MetadataError::InvalidProviderResponse);
            }
            let label = tag
                .display_names
                .iter()
                .find(|name| name.language == "ko")
                .or_else(|| tag.display_names.iter().find(|name| name.language == "en"))
                .map(|name| name.name.clone())
                .unwrap_or_else(|| tag.key.clone());
            tags.insert(
                tag.key.clone(),
                NormalizedTag {
                    tag: tag.key,
                    label,
                },
            );
        }
        let hash = Sha256::digest(payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(NormalizedMetadata {
            raw_level: decoded.level,
            normalized_tier: tier_token(decoded.level),
            tags: tags.into_values().collect(),
            response_hash: hash,
            payload: serde_json::from_slice(payload)
                .map_err(|_| MetadataError::InvalidProviderResponse)?,
        })
    }
}

pub fn tier_token(level: i16) -> String {
    if level == 0 {
        return "unrated".to_owned();
    }
    let division = match level {
        1..=5 => "bronze",
        6..=10 => "silver",
        11..=15 => "gold",
        16..=20 => "platinum",
        21..=25 => "diamond",
        26..=30 => "ruby",
        _ => return format!("special-{level}"),
    };
    let numeral = ["v", "iv", "iii", "ii", "i"][(usize::from(level as u16) - 1) % 5];
    format!("{division}-{numeral}")
}

fn valid_external_id(value: &str) -> bool {
    (1..=12).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn allowed_endpoint(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host_str() {
        Some("solved.ac") => url.scheme() == "https",
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}

async fn store_success(
    pool: &PgPool,
    provider: &impl ProblemMetadataProvider,
    external_problem_id: &str,
    source_url: &str,
    metadata: NormalizedMetadata,
) -> Result<(), MetadataError> {
    sqlx::query(
        r#"
        INSERT INTO external_problem_metadata_cache (
            provider, external_problem_id, payload, fetched_at, expires_at, source_url, state,
            raw_level, normalized_tier, normalized_tags, response_hash, provider_revision,
            last_attempt_at, last_error
        ) VALUES ($1, $2, $3, now(), now() + interval '7 days', $4, 'fresh',
                  $5, $6, $7, $8, $9, now(), NULL)
        ON CONFLICT (provider, external_problem_id) DO UPDATE
        SET payload = EXCLUDED.payload,
            fetched_at = EXCLUDED.fetched_at,
            expires_at = EXCLUDED.expires_at,
            source_url = EXCLUDED.source_url,
            state = 'fresh',
            raw_level = EXCLUDED.raw_level,
            normalized_tier = EXCLUDED.normalized_tier,
            normalized_tags = EXCLUDED.normalized_tags,
            response_hash = EXCLUDED.response_hash,
            provider_revision = EXCLUDED.provider_revision,
            last_attempt_at = now(),
            last_error = NULL
        "#,
    )
    .bind(provider.provider())
    .bind(external_problem_id)
    .bind(metadata.payload)
    .bind(source_url)
    .bind(metadata.raw_level)
    .bind(metadata.normalized_tier)
    .bind(serde_json::to_value(metadata.tags).map_err(|_| MetadataError::InvalidProviderResponse)?)
    .bind(metadata.response_hash)
    .bind(provider.revision())
    .execute(pool)
    .await?;
    Ok(())
}

async fn store_failure(
    pool: &PgPool,
    provider: &impl ProblemMetadataProvider,
    external_problem_id: &str,
    source_url: &str,
    error: &MetadataError,
) -> Result<(), MetadataError> {
    sqlx::query(
        r#"
        INSERT INTO external_problem_metadata_cache (
            provider, external_problem_id, payload, fetched_at, expires_at, source_url, state,
            normalized_tags, provider_revision, last_attempt_at, last_error
        ) VALUES ($1, $2, '{}'::jsonb, now(), now(), $3, 'blocked', '[]'::jsonb, $4, now(), $5)
        ON CONFLICT (provider, external_problem_id) DO UPDATE
        SET state = CASE
                WHEN external_problem_metadata_cache.raw_level IS NULL THEN 'blocked'
                ELSE 'stale'
            END,
            source_url = EXCLUDED.source_url,
            provider_revision = EXCLUDED.provider_revision,
            last_attempt_at = now(),
            last_error = EXCLUDED.last_error
        "#,
    )
    .bind(provider.provider())
    .bind(external_problem_id)
    .bind(source_url)
    .bind(provider.revision())
    .bind(error.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn refresh_solved_ac_at(
    pool: &PgPool,
    client: &reqwest::Client,
    external_problem_id: &str,
    endpoint: &str,
) -> Result<(), MetadataError> {
    if !valid_external_id(external_problem_id) {
        return Err(MetadataError::InvalidExternalId);
    }
    let mut url = Url::parse(endpoint).map_err(|_| MetadataError::InvalidEndpoint)?;
    if !allowed_endpoint(&url) {
        return Err(MetadataError::InvalidEndpoint);
    }
    url.query_pairs_mut()
        .append_pair("problemId", external_problem_id);
    let provider = SolvedAcV3;
    let source_url = url.to_string();
    let jitter = u64::from(Sha256::digest(external_problem_id.as_bytes())[0] % 25);
    let mut final_error = MetadataError::ProviderRequest;

    for attempt in 0_u64..3 {
        match client
            .get(url.clone())
            .header("accept", "application/json")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let payload = match response.bytes().await {
                    Ok(payload) => payload,
                    Err(_) => {
                        final_error = MetadataError::ProviderRequest;
                        break;
                    }
                };
                let metadata = match provider.normalize(&payload) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        final_error = error;
                        break;
                    }
                };
                return store_success(pool, &provider, external_problem_id, &source_url, metadata)
                    .await;
            }
            Ok(response) => {
                let status = response.status();
                final_error = MetadataError::ProviderStatus(status);
                if !(status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) {
                    break;
                }
            }
            Err(_) => final_error = MetadataError::ProviderRequest,
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(
                50 * 2_u64.pow(attempt as u32) + jitter,
            ))
            .await;
        }
    }

    store_failure(
        pool,
        &provider,
        external_problem_id,
        &source_url,
        &final_error,
    )
    .await?;
    Err(final_error)
}

pub async fn refresh_solved_ac_metadata(
    pool: &PgPool,
    client: &reqwest::Client,
    external_problem_id: &str,
) -> Result<(), MetadataError> {
    refresh_solved_ac_at(pool, client, external_problem_id, SOLVED_AC_V3_ENDPOINT).await
}

#[derive(Debug)]
pub enum MetadataHttpError {
    Auth(crate::auth::AuthError),
    Forbidden,
    Disabled,
    Refresh(MetadataError),
    Database(sqlx::Error),
}

impl IntoResponse for MetadataHttpError {
    fn into_response(self) -> Response {
        match self {
            Self::Auth(error) => error.into_response(),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": {"code": "admin_required", "message": "관리자 권한이 필요합니다"}})),
            )
                .into_response(),
            Self::Disabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": {"code": "solved_ac_disabled", "message": "solved.ac 실시간 동기화는 현재 비활성화되어 있습니다"}})),
            )
                .into_response(),
            Self::Refresh(error) => {
                tracing::warn!(%error, "외부 문제 메타데이터 갱신 실패");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": {"code": "metadata_refresh_failed", "message": "외부 메타데이터를 갱신하지 못했습니다. 마지막 정상 값은 보존됩니다"}})),
                )
                    .into_response()
            }
            Self::Database(error) => {
                tracing::error!(%error, "메타데이터 관리자 작업 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"code": "internal_error", "message": "요청을 처리하지 못했습니다"}})),
                )
                    .into_response()
            }
        }
    }
}

impl From<crate::auth::AuthError> for MetadataHttpError {
    fn from(error: crate::auth::AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for MetadataHttpError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub async fn admin_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(external_problem_id): Path<String>,
) -> Result<Json<serde_json::Value>, MetadataHttpError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role = 'ADMIN')",
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !is_admin {
        return Err(MetadataHttpError::Forbidden);
    }
    if !state.settings().solved_ac_enabled {
        return Err(MetadataHttpError::Disabled);
    }
    refresh_solved_ac_metadata(state.pool(), state.http_client(), &external_problem_id)
        .await
        .map_err(MetadataHttpError::Refresh)?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'metadata.solved_ac.refreshed', 'external_problem', $2)",
    )
    .bind(user_id)
    .bind(&external_problem_id)
    .execute(state.pool())
    .await?;
    Ok(Json(serde_json::json!({
        "provider": "solved_ac",
        "external_problem_id": external_problem_id,
        "state": "fresh"
    })))
}
