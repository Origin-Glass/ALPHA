use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::http::AppState;

const SESSION_COOKIE: &str = "alpha_session";
const SESSION_SECONDS: i64 = 60 * 60 * 24 * 30;

#[derive(Debug, Deserialize)]
pub struct TestSessionRequest {
    handle: String,
}

#[derive(Debug, Deserialize)]
pub struct TermsRequest {
    version: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserView {
    id: Uuid,
    handle: String,
    display_name: String,
    terms_accepted: bool,
    roles: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    user: UserView,
    csrf_token: String,
}

#[derive(Debug)]
pub enum AuthError {
    InvalidInput(&'static str),
    Unauthorized,
    Forbidden,
    TestIdentityDisabled,
    ProviderUnavailable,
    UnsupportedProvider,
    InvalidOauthState,
    OauthExchange,
    EmailCollision,
    Database(sqlx::Error),
    Randomness,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "로그인이 필요합니다",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "csrf_rejected",
                "요청 검증에 실패했습니다",
            ),
            Self::TestIdentityDisabled => (
                StatusCode::NOT_FOUND,
                "not_found",
                "요청한 기능을 찾을 수 없습니다",
            ),
            Self::ProviderUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "oauth_provider_unavailable",
                "OAuth 제공자 설정이 필요합니다",
            ),
            Self::UnsupportedProvider => (
                StatusCode::NOT_FOUND,
                "oauth_provider_not_found",
                "지원하지 않는 OAuth 제공자입니다",
            ),
            Self::InvalidOauthState => (
                StatusCode::BAD_REQUEST,
                "oauth_state_invalid",
                "로그인 요청이 만료됐거나 이미 사용됐습니다",
            ),
            Self::OauthExchange => (
                StatusCode::BAD_GATEWAY,
                "oauth_exchange_failed",
                "OAuth 제공자 응답을 확인하지 못했습니다",
            ),
            Self::EmailCollision => (
                StatusCode::CONFLICT,
                "oauth_account_link_required",
                "같은 이메일의 기존 계정이 있습니다. 로그인 후 계정 연결이 필요합니다",
            ),
            Self::Database(error) => {
                tracing::error!(%error, "인증 데이터베이스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
            Self::Randomness => {
                tracing::error!("안전한 인증 토큰 생성 실패");
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

impl From<sqlx::Error> for AuthError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

fn random_token() -> Result<String, AuthError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AuthError::Randomness)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[derive(Debug, Deserialize)]
pub struct OAuthStartQuery {
    redirect_after: Option<String>,
}

#[derive(Debug, FromRow)]
pub struct OAuthTransaction {
    pub pkce_verifier: String,
    pub redirect_after: String,
}

#[derive(Debug)]
pub struct OAuthProfile {
    pub provider: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub display_name: String,
    pub handle_hint: String,
}

#[derive(Debug)]
pub enum IdentityResolutionError {
    EmailCollision,
    UnsupportedProvider,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for IdentityResolutionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

struct OAuthProvider<'a> {
    name: &'static str,
    client_id: &'a str,
    client_secret: &'a str,
    authorization_url: &'static str,
    token_url: &'static str,
    scope: &'static str,
}

fn oauth_provider<'a>(state: &'a AppState, provider: &str) -> Result<OAuthProvider<'a>, AuthError> {
    match provider {
        "google" if !state.settings().google_client_id.is_empty() => Ok(OAuthProvider {
            name: "google",
            client_id: &state.settings().google_client_id,
            client_secret: &state.settings().google_client_secret,
            authorization_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scope: "openid email profile",
        }),
        "github" if !state.settings().github_client_id.is_empty() => Ok(OAuthProvider {
            name: "github",
            client_id: &state.settings().github_client_id,
            client_secret: &state.settings().github_client_secret,
            authorization_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            scope: "read:user user:email",
        }),
        "google" | "github" => Err(AuthError::ProviderUnavailable),
        _ => Err(AuthError::UnsupportedProvider),
    }
}

fn safe_redirect_after(value: Option<String>) -> Result<String, AuthError> {
    match value {
        Some(value) if value.starts_with('/') && !value.starts_with("//") => Ok(value),
        Some(_) => Err(AuthError::InvalidInput("이동 경로가 올바르지 않습니다")),
        None => Ok("/".to_owned()),
    }
}

fn safe_handle_base(hint: &str) -> String {
    let normalized: String = hint
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(20)
        .collect();
    let trimmed = normalized.trim_matches('-');
    if trimmed.len() >= 3 {
        trimmed.to_owned()
    } else {
        "user".to_owned()
    }
}

pub async fn resolve_oauth_identity(
    pool: &sqlx::PgPool,
    profile: OAuthProfile,
) -> Result<Uuid, IdentityResolutionError> {
    if !matches!(profile.provider.as_str(), "google" | "github") {
        return Err(IdentityResolutionError::UnsupportedProvider);
    }

    let mut transaction = pool.begin().await?;
    let existing_user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM oauth_identities WHERE provider = $1 AND subject = $2",
    )
    .bind(&profile.provider)
    .bind(&profile.subject)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(user_id) = existing_user_id {
        sqlx::query(
            "UPDATE oauth_identities SET last_login_at = now() WHERE provider = $1 AND subject = $2",
        )
        .bind(&profile.provider)
        .bind(&profile.subject)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok(user_id);
    }

    let trusted_email = profile.email.filter(|_| profile.email_verified);
    if let Some(email) = trusted_email.as_deref() {
        let email_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1) AND status <> 'deleted')",
        )
        .bind(email)
        .fetch_one(&mut *transaction)
        .await?;
        if email_exists {
            return Err(IdentityResolutionError::EmailCollision);
        }
    }

    let user_id = Uuid::now_v7();
    let suffix = &user_id.simple().to_string()[..8];
    let handle = format!("{}-{suffix}", safe_handle_base(&profile.handle_hint));
    let display_name = if profile.display_name.trim().is_empty() {
        handle.clone()
    } else {
        profile.display_name.chars().take(80).collect()
    };
    let insert_user = sqlx::query(
        r#"
        INSERT INTO users (id, handle, display_name, email, email_verified)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(user_id)
    .bind(&handle)
    .bind(display_name)
    .bind(trusted_email.as_deref())
    .bind(trusted_email.is_some())
    .execute(&mut *transaction)
    .await;
    if let Err(error) = insert_user {
        if error
            .as_database_error()
            .and_then(|database_error| database_error.constraint())
            == Some("users_normalized_email_idx")
        {
            return Err(IdentityResolutionError::EmailCollision);
        }
        return Err(IdentityResolutionError::Database(error));
    }

    sqlx::query(
        r#"
        INSERT INTO oauth_identities
            (provider, subject, user_id, email_at_link, email_verified_at_link)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(&profile.provider)
    .bind(&profile.subject)
    .bind(user_id)
    .bind(trusted_email.as_deref())
    .bind(trusted_email.is_some())
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'USER')")
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'oauth.identity.created', 'user', $2)",
    )
    .bind(user_id)
    .bind(user_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(user_id)
}

pub async fn oauth_start(
    State(state): State<AppState>,
    Path(provider_name): Path<String>,
    Query(query): Query<OAuthStartQuery>,
) -> Result<Redirect, AuthError> {
    let provider = oauth_provider(&state, &provider_name)?;
    let redirect_after = safe_redirect_after(query.redirect_after)?;
    let oauth_state = random_token()?;
    let pkce_verifier = random_token()?;
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));

    sqlx::query(
        r#"
        INSERT INTO oauth_transactions
            (state_hash, provider, pkce_verifier, redirect_after, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(token_hash(&oauth_state))
    .bind(provider.name)
    .bind(&pkce_verifier)
    .bind(&redirect_after)
    .bind(OffsetDateTime::now_utc() + Duration::minutes(10))
    .execute(state.pool())
    .await?;

    let callback_url = format!(
        "{}/api/v1/auth/{}/callback",
        state.settings().public_base_url.trim_end_matches('/'),
        provider.name
    );
    let mut authorization_url =
        Url::parse(provider.authorization_url).expect("고정 OAuth URL은 유효하다");
    authorization_url
        .query_pairs_mut()
        .append_pair("client_id", provider.client_id)
        .append_pair("redirect_uri", &callback_url)
        .append_pair("response_type", "code")
        .append_pair("scope", provider.scope)
        .append_pair("state", &oauth_state)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256");

    Ok(Redirect::to(authorization_url.as_str()))
}

pub async fn consume_oauth_transaction(
    pool: &sqlx::PgPool,
    provider: &str,
    state: &str,
) -> Result<Option<OAuthTransaction>, sqlx::Error> {
    sqlx::query_as::<_, OAuthTransaction>(
        r#"
        UPDATE oauth_transactions
        SET used_at = now()
        WHERE state_hash = $1 AND provider = $2 AND used_at IS NULL AND expires_at > now()
        RETURNING pkce_verifier, redirect_after
        "#,
    )
    .bind(token_hash(state))
    .bind(provider)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubUserInfo {
    id: u64,
    login: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Debug, Serialize)]
struct ProviderStatus {
    id: &'static str,
    label: &'static str,
    available: bool,
    verification: &'static str,
}

pub async fn providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    let status = |id, label, available| ProviderStatus {
        id,
        label,
        available,
        verification: if available {
            "implemented_unverified"
        } else {
            "blocked_missing_credentials"
        },
    };
    Json(serde_json::json!({
        "providers": [
            status("google", "Google", !state.settings().google_client_id.is_empty()),
            status("github", "GitHub", !state.settings().github_client_id.is_empty())
        ],
        "test_identity_available": state.settings().test_identity_enabled
            && state.settings().app_env != "production"
    }))
}

async fn exchange_oauth_code(
    state: &AppState,
    provider: &OAuthProvider<'_>,
    code: &str,
    verifier: &str,
) -> Result<String, AuthError> {
    let redirect_uri = format!(
        "{}/api/v1/auth/{}/callback",
        state.settings().public_base_url.trim_end_matches('/'),
        provider.name
    );
    let response = state
        .http_client()
        .post(provider.token_url)
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", provider.client_id),
            ("client_secret", provider.client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|_| AuthError::OauthExchange)?;
    if !response.status().is_success() {
        return Err(AuthError::OauthExchange);
    }
    let token = response
        .json::<TokenResponse>()
        .await
        .map_err(|_| AuthError::OauthExchange)?;
    if token.access_token.is_empty() {
        return Err(AuthError::OauthExchange);
    }
    Ok(token.access_token)
}

async fn fetch_oauth_profile(
    state: &AppState,
    provider: &str,
    access_token: &str,
) -> Result<OAuthProfile, AuthError> {
    match provider {
        "google" => {
            let response = state
                .http_client()
                .get("https://openidconnect.googleapis.com/v1/userinfo")
                .bearer_auth(access_token)
                .send()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            if !response.status().is_success() {
                return Err(AuthError::OauthExchange);
            }
            let profile = response
                .json::<GoogleUserInfo>()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            let handle_hint = profile
                .email
                .as_deref()
                .and_then(|email| email.split('@').next())
                .unwrap_or("google-user")
                .to_owned();
            Ok(OAuthProfile {
                provider: "google".to_owned(),
                subject: profile.sub,
                email: profile.email,
                email_verified: profile.email_verified.unwrap_or(false),
                display_name: profile.name.unwrap_or_else(|| "Google 사용자".to_owned()),
                handle_hint,
            })
        }
        "github" => {
            let request = |url: &'static str| {
                state
                    .http_client()
                    .get(url)
                    .bearer_auth(access_token)
                    .header(header::ACCEPT, "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
            };
            let user_response = request("https://api.github.com/user")
                .send()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            if !user_response.status().is_success() {
                return Err(AuthError::OauthExchange);
            }
            let user = user_response
                .json::<GithubUserInfo>()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            let email_response = request("https://api.github.com/user/emails")
                .send()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            if !email_response.status().is_success() {
                return Err(AuthError::OauthExchange);
            }
            let emails = email_response
                .json::<Vec<GithubEmail>>()
                .await
                .map_err(|_| AuthError::OauthExchange)?;
            let verified_email = emails
                .iter()
                .find(|email| email.primary && email.verified)
                .or_else(|| emails.iter().find(|email| email.verified))
                .map(|email| email.email.clone());
            Ok(OAuthProfile {
                provider: "github".to_owned(),
                subject: user.id.to_string(),
                email_verified: verified_email.is_some(),
                email: verified_email,
                display_name: user.name.unwrap_or_else(|| user.login.clone()),
                handle_hint: user.login,
            })
        }
        _ => Err(AuthError::UnsupportedProvider),
    }
}

pub async fn oauth_callback(
    State(state): State<AppState>,
    Path(provider_name): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Response, AuthError> {
    if query.error.is_some() {
        return Err(AuthError::InvalidInput("OAuth 로그인이 취소됐습니다"));
    }
    let code = query
        .code
        .filter(|code| !code.is_empty() && code.len() <= 2048)
        .ok_or(AuthError::InvalidInput("OAuth 인증 코드가 없습니다"))?;
    let oauth_state = query
        .state
        .filter(|state| !state.is_empty() && state.len() <= 256)
        .ok_or(AuthError::InvalidOauthState)?;
    let provider = oauth_provider(&state, &provider_name)?;
    let transaction = consume_oauth_transaction(state.pool(), provider.name, &oauth_state)
        .await?
        .ok_or(AuthError::InvalidOauthState)?;
    let access_token =
        exchange_oauth_code(&state, &provider, &code, &transaction.pkce_verifier).await?;
    let profile = fetch_oauth_profile(&state, provider.name, &access_token).await?;
    let user_id = resolve_oauth_identity(state.pool(), profile)
        .await
        .map_err(|error| match error {
            IdentityResolutionError::EmailCollision => AuthError::EmailCollision,
            IdentityResolutionError::UnsupportedProvider => AuthError::UnsupportedProvider,
            IdentityResolutionError::Database(error) => AuthError::Database(error),
        })?;
    let (headers, _, _) = issue_session(&state, user_id).await?;
    Ok((headers, Redirect::to(&transaction.redirect_after)).into_response())
}

fn valid_test_subject(subject: &str) -> bool {
    (3..=18).contains(&subject.len())
        && subject
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !subject.starts_with('-')
        && !subject.ends_with('-')
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

async fn user_view(state: &AppState, user_id: Uuid) -> Result<UserView, AuthError> {
    sqlx::query_as::<_, UserView>(
        r#"
        SELECT u.id, u.handle, u.display_name, u.terms_accepted_at IS NOT NULL AS terms_accepted,
               COALESCE(array_agg(ur.role ORDER BY ur.role) FILTER (WHERE ur.role IS NOT NULL), ARRAY[]::text[]) AS roles
        FROM users u
        LEFT JOIN user_roles ur ON ur.user_id = u.id
        WHERE u.id = $1 AND u.status = 'active'
        GROUP BY u.id
        "#,
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)
}

async fn issue_session(
    state: &AppState,
    user_id: Uuid,
) -> Result<(HeaderMap, String, UserView), AuthError> {
    let session_token = random_token()?;
    let csrf_token = random_token()?;
    sqlx::query(
        r#"
        INSERT INTO sessions (user_id, session_token_hash, csrf_token_hash, expires_at)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(token_hash(&session_token))
    .bind(token_hash(&csrf_token))
    .bind(OffsetDateTime::now_utc() + Duration::seconds(SESSION_SECONDS))
    .execute(state.pool())
    .await?;

    let user = user_view(state, user_id).await?;
    let secure = if state.settings().app_env == "production" {
        "; Secure"
    } else {
        ""
    };
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE}={session_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECONDS}{secure}"
        ))
        .expect("생성된 토큰은 유효한 쿠키다"),
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "alpha_csrf={csrf_token}; Path=/; SameSite=Strict; Max-Age={SESSION_SECONDS}{secure}"
        ))
        .expect("생성된 토큰은 유효한 쿠키다"),
    );
    Ok((headers, csrf_token, user))
}

pub async fn test_session(
    State(state): State<AppState>,
    Json(request): Json<TestSessionRequest>,
) -> Result<Response, AuthError> {
    if state.settings().app_env == "production" || !state.settings().test_identity_enabled {
        return Err(AuthError::TestIdentityDisabled);
    }
    if !valid_test_subject(&request.handle) {
        return Err(AuthError::InvalidInput(
            "테스트 식별자는 3~18자의 영문 소문자, 숫자, 하이픈만 사용할 수 있습니다",
        ));
    }

    let mut transaction = state.pool().begin().await?;
    let existing_user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM oauth_identities WHERE provider = 'test' AND subject = $1",
    )
    .bind(&request.handle)
    .fetch_optional(&mut *transaction)
    .await?;

    let user_id = if let Some(user_id) = existing_user_id {
        user_id
    } else {
        let user_id = Uuid::now_v7();
        let suffix = &user_id.simple().to_string()[..6];
        let handle = format!("test-{}-{suffix}", request.handle);
        sqlx::query("INSERT INTO users (id, handle, display_name) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(handle)
            .bind(&request.handle)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO oauth_identities (provider, subject, user_id) VALUES ('test', $1, $2)",
        )
        .bind(&request.handle)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'USER')")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        user_id
    };

    transaction.commit().await?;
    let (headers, csrf_token, user) = issue_session(&state, user_id).await?;

    Ok((
        StatusCode::CREATED,
        headers,
        Json(SessionResponse { user, csrf_token }),
    )
        .into_response())
}

pub async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<UserView>, AuthError> {
    let user_id = authenticated_user_id(&state, &headers).await?;
    Ok(Json(user_view(&state, user_id).await?))
}

pub async fn authenticated_user_id(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Uuid, AuthError> {
    let session_token = cookie_value(headers, SESSION_COOKIE).ok_or(AuthError::Unauthorized)?;
    let user_id: Uuid = sqlx::query_scalar(
        r#"
        SELECT user_id FROM sessions
        WHERE session_token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
        "#,
    )
    .bind(token_hash(&session_token))
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    Ok(user_id)
}

pub async fn authenticated_user_id_with_csrf(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Uuid, AuthError> {
    let session_token = cookie_value(headers, SESSION_COOKIE).ok_or(AuthError::Unauthorized)?;
    let csrf_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::Forbidden)?;
    let session: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT user_id, csrf_token_hash FROM sessions
        WHERE session_token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
        "#,
    )
    .bind(token_hash(&session_token))
    .fetch_optional(state.pool())
    .await?;
    let (user_id, stored_csrf_hash) = session.ok_or(AuthError::Unauthorized)?;
    if !bool::from(stored_csrf_hash.ct_eq(&token_hash(csrf_token))) {
        return Err(AuthError::Forbidden);
    }
    Ok(user_id)
}

pub async fn accept_terms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TermsRequest>,
) -> Result<Json<UserView>, AuthError> {
    if request.version != "2026-07-18" {
        return Err(AuthError::InvalidInput("지원하지 않는 약관 버전입니다"));
    }
    let session_token = cookie_value(&headers, SESSION_COOKIE).ok_or(AuthError::Unauthorized)?;
    let csrf_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::Forbidden)?;
    let session: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT user_id, csrf_token_hash FROM sessions
        WHERE session_token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
        "#,
    )
    .bind(token_hash(&session_token))
    .fetch_optional(state.pool())
    .await?;
    let (user_id, stored_csrf_hash) = session.ok_or(AuthError::Unauthorized)?;
    if !bool::from(stored_csrf_hash.ct_eq(&token_hash(csrf_token))) {
        return Err(AuthError::Forbidden);
    }

    let mut transaction = state.pool().begin().await?;
    sqlx::query(
        "UPDATE users SET terms_accepted_at = now(), terms_accepted_version = $2, updated_at = now() WHERE id = $1",
    )
    .bind(user_id)
    .bind(&request.version)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'terms.accepted', 'user', $2, jsonb_build_object('version', $3::text))",
    )
    .bind(user_id)
    .bind(user_id.to_string())
    .bind(&request.version)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(Json(user_view(&state, user_id).await?))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let session_token = cookie_value(&headers, SESSION_COOKIE).ok_or(AuthError::Unauthorized)?;
    let csrf_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::Forbidden)?;
    let session_hash = token_hash(&session_token);
    let stored_csrf_hash: Vec<u8> = sqlx::query_scalar(
        r#"
        SELECT csrf_token_hash FROM sessions
        WHERE session_token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
        "#,
    )
    .bind(&session_hash)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;

    if !bool::from(stored_csrf_hash.ct_eq(&token_hash(csrf_token))) {
        return Err(AuthError::Forbidden);
    }

    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE session_token_hash = $1")
        .bind(session_hash)
        .execute(state.pool())
        .await?;

    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("alpha_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("alpha_csrf=; Path=/; SameSite=Strict; Max-Age=0"),
    );
    Ok((StatusCode::NO_CONTENT, headers).into_response())
}
