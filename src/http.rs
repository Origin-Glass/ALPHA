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
    ai_provider: Arc<dyn crate::activities::AiAssistanceProvider>,
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
            ai_provider: crate::activities::default_ai_provider(),
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

    pub fn ai_provider(&self) -> &dyn crate::activities::AiAssistanceProvider {
        self.ai_provider.as_ref()
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
        .route("/api/v1/onboarding", get(crate::onboarding::questions))
        .route(
            "/api/v1/onboarding/complete",
            axum::routing::post(crate::onboarding::complete),
        )
        .route(
            "/api/v1/learning/path",
            get(crate::onboarding::learning_path),
        )
        .route("/api/v1/activities", get(crate::activities::list))
        .route("/api/v1/activities/{slug}", get(crate::activities::detail))
        .route(
            "/api/v1/activities/{slug}/start",
            axum::routing::post(crate::activities::start),
        )
        .route(
            "/api/v1/activities/{slug}/attempts",
            axum::routing::post(crate::activities::attempt),
        )
        .route(
            "/api/v1/activities/{slug}/assistance/{level}",
            axum::routing::post(crate::activities::assistance),
        )
        .route(
            "/api/v1/assistance/status",
            get(crate::activities::ai_status),
        )
        .route("/api/v1/tracks/{slug}", get(crate::activities::track))
        .route(
            "/api/v1/admin/metadata/solved-ac/{external_problem_id}/refresh",
            axum::routing::post(crate::metadata::admin_refresh),
        )
        .route(
            "/api/v1/payments/checkout",
            axum::routing::post(crate::payments::create_checkout),
        )
        .route("/api/v1/classes/{class_id}", get(crate::classes::detail))
        .route("/api/v1/problems", get(crate::problems::list))
        .route("/api/v1/problems/{slug}", get(crate::problems::detail))
        .route(
            "/api/v1/submissions",
            get(crate::submissions::list).post(crate::submissions::create),
        )
        .route(
            "/api/v1/runs",
            axum::routing::post(crate::submissions::create_run),
        )
        .route(
            "/api/v1/submissions/{submission_id}",
            get(crate::submissions::detail),
        )
        .route(
            "/api/v1/submissions/{submission_id}/cancel",
            axum::routing::post(crate::submissions::cancel),
        )
        .route(
            "/api/v1/submissions/{submission_id}/events",
            get(crate::submissions::events),
        )
        .route(
            "/api/v1/problems/{slug}/draft",
            get(crate::submissions::get_draft).put(crate::submissions::save_draft),
        )
        .route(
            "/api/v1/admin/problems/{slug}/rejudge",
            axum::routing::post(crate::submissions::rejudge_problem),
        )
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
