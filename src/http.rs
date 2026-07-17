use axum::{Json, Router, http::StatusCode, routing::get};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::config::Settings;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    settings: Arc<Settings>,
    http_client: reqwest::Client,
}

impl AppState {
    pub fn new(pool: PgPool, settings: Settings) -> Self {
        Self {
            pool,
            settings: Arc::new(settings),
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Origin-Glass-ALPHA/0.1")
                .build()
                .expect("고정 HTTP 클라이언트 설정은 유효하다"),
        }
    }

    pub fn for_test(pool: PgPool) -> Self {
        let settings = Settings::from_pairs(std::collections::HashMap::<String, String>::new())
            .expect("빈 테스트 설정은 유효하다");
        Self::new(pool, settings)
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route(
            "/api/v1/auth/test-session",
            axum::routing::post(crate::auth::test_session),
        )
        .route("/api/v1/auth/me", get(crate::auth::me))
        .route(
            "/api/v1/auth/terms",
            axum::routing::post(crate::auth::accept_terms),
        )
        .route(
            "/api/v1/auth/{provider}/start",
            get(crate::auth::oauth_start),
        )
        .route(
            "/api/v1/auth/{provider}/callback",
            get(crate::auth::oauth_callback),
        )
        .route("/api/v1/auth/providers", get(crate::auth::providers))
        .route(
            "/api/v1/auth/logout",
            axum::routing::post(crate::auth::logout),
        )
        .route(
            "/api/v1/payments/checkout",
            axum::routing::post(crate::payments::create_checkout),
        )
        .route("/api/v1/classes/{class_id}", get(crate::classes::detail))
        .route("/api/v1/problems", get(crate::problems::list))
        .route("/api/v1/problems/{slug}", get(crate::problems::detail))
        .with_state(state)
}

async fn liveness() -> Json<Value> {
    Json(json!({"service": "alpha-api", "status": "ok"}))
}

async fn readiness(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> (StatusCode, Json<Value>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.pool())
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"service": "alpha-api", "status": "ready"})),
        ),
        Err(error) => {
            tracing::warn!(%error, "준비 상태 확인 실패");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"service": "alpha-api", "status": "unavailable"})),
            )
        }
    }
}
