use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::config::Settings;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    settings: Arc<Settings>,
    http_client: reqwest::Client,
    ai_provider: Arc<dyn crate::activities::AiAssistanceProvider>,
    runtime: Arc<crate::observability::RuntimeMetrics>,
}

impl AppState {
    pub fn new(pool: PgPool, settings: Settings) -> Self {
        Self {
            pool,
            settings: Arc::new(settings),
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Origin-Glass-ALPHA/1.0")
                .build()
                .expect("고정 HTTP 클라이언트 설정은 유효하다"),
            ai_provider: crate::activities::default_ai_provider(),
            runtime: Arc::new(crate::observability::RuntimeMetrics::default()),
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

    pub fn runtime(&self) -> &crate::observability::RuntimeMetrics {
        &self.runtime
    }

    pub fn runtime_handle(&self) -> Arc<crate::observability::RuntimeMetrics> {
        Arc::clone(&self.runtime)
    }
}

pub fn router(state: AppState) -> Router {
    let middleware_state = state.clone();
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/metrics", get(metrics))
        .route(
            "/api/v1/auth/test-session",
            axum::routing::post(crate::auth::test_session),
        )
        .route("/api/v1/auth/me", get(crate::auth::me))
        .route(
            "/api/v1/auth/terms",
            axum::routing::post(crate::auth::accept_terms),
        )
        .route("/api/v1/policies", get(crate::auth::policies))
        .route(
            "/api/v1/policies/consents",
            axum::routing::post(crate::auth::accept_policy),
        )
        .route(
            "/api/v1/data-requests",
            axum::routing::post(crate::governance::create_data_request),
        )
        .route(
            "/api/v1/data-requests/{id}",
            get(crate::governance::data_request),
        )
        .route(
            "/api/v1/data-requests/{id}/cancel",
            axum::routing::post(crate::governance::cancel_data_request),
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
            "/api/v1/progression/dashboard",
            get(crate::gamification::dashboard),
        )
        .route(
            "/api/v1/profile",
            axum::routing::put(crate::gamification::update_profile),
        )
        .route(
            "/api/v1/profiles/{handle}",
            get(crate::gamification::profile),
        )
        .route("/api/v1/rankings", get(crate::gamification::rankings))
        .route(
            "/api/v1/community",
            get(crate::community::list).post(crate::community::create_post),
        )
        .route("/api/v1/community/{post_id}", get(crate::community::detail))
        .route(
            "/api/v1/community/{post_id}/answers",
            axum::routing::post(crate::community::create_answer),
        )
        .route(
            "/api/v1/community/{post_id}/answers/{answer_id}/accept",
            axum::routing::post(crate::community::accept_answer),
        )
        .route(
            "/api/v1/community/reports",
            axum::routing::post(crate::community::create_report),
        )
        .route("/api/v1/contests", get(crate::contests::list))
        .route("/api/v1/contests/{slug}", get(crate::contests::detail))
        .route(
            "/api/v1/contests/{slug}/join",
            axum::routing::post(crate::contests::join),
        )
        .route(
            "/api/v1/contests/{slug}/scoreboard",
            get(crate::contests::scoreboard),
        )
        .route(
            "/api/v1/admin/contests",
            axum::routing::post(crate::contests::create),
        )
        .route(
            "/api/v1/admin/contests/{slug}/finalize",
            axum::routing::post(crate::contests::finalize),
        )
        .route(
            "/api/v1/admin/metadata/solved-ac/{external_problem_id}/refresh",
            axum::routing::post(crate::metadata::admin_refresh),
        )
        .route(
            "/api/v1/admin/problems",
            axum::routing::post(crate::admin::create_problem),
        )
        .route(
            "/api/v1/admin/problems/{slug}/revisions",
            axum::routing::post(crate::admin::revise_problem),
        )
        .route(
            "/api/v1/governance/problems/{slug}/rights",
            axum::routing::post(crate::governance::record_rights),
        )
        .route(
            "/api/v1/governance/problems/{slug}/reviews/content",
            axum::routing::post(crate::governance::approve_content),
        )
        .route(
            "/api/v1/governance/problems/{slug}/reviews/rights",
            axum::routing::post(crate::governance::approve_rights),
        )
        .route(
            "/api/v1/governance/problems/{slug}/publish",
            axum::routing::post(crate::governance::publish),
        )
        .route(
            "/api/v1/admin/notices",
            axum::routing::post(crate::community::create_notice),
        )
        .route(
            "/api/v1/admin/community/reports",
            get(crate::community::report_queue),
        )
        .route(
            "/api/v1/admin/community/reports/{report_id}/resolve",
            axum::routing::post(crate::community::resolve_report),
        )
        .route("/api/v1/admin/judge/workers", get(crate::admin::workers))
        .route("/api/v1/admin/audit", get(crate::admin::audit_log))
        .route(
            "/api/v1/payments/checkout",
            axum::routing::post(crate::payments::create_checkout),
        )
        .route(
            "/api/v1/organizations",
            axum::routing::post(crate::classes::create_organization),
        )
        .route(
            "/api/v1/organizations/{organization_id}/classes",
            axum::routing::post(crate::classes::create_class),
        )
        .route("/api/v1/classes", get(crate::classes::list))
        .route("/api/v1/classes/{class_id}", get(crate::classes::detail))
        .route(
            "/api/v1/classes/{class_id}/invitations",
            axum::routing::post(crate::classes::create_invitation),
        )
        .route(
            "/api/v1/class-invitations/accept",
            axum::routing::post(crate::classes::accept_invitation),
        )
        .route(
            "/api/v1/classes/{class_id}/assignments",
            axum::routing::post(crate::classes::create_assignment),
        )
        .route(
            "/api/v1/classes/{class_id}/instructor-dashboard",
            get(crate::classes::instructor_dashboard),
        )
        .route(
            "/api/v1/classes/{class_id}/export.csv",
            get(crate::classes::export_csv),
        )
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
        .layer(middleware::from_fn_with_state(
            middleware_state,
            crate::observability::track_and_limit,
        ))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<Body>| {
                    let request_id = request
                        .extensions()
                        .get::<RequestId>()
                        .and_then(|id| id.header_value().to_str().ok())
                        .unwrap_or("unknown");
                    tracing::info_span!(
                        "http_request",
                        %request_id,
                        method = %request.method(),
                        uri = %request.uri()
                    )
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
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

async fn metrics(State(state): State<AppState>) -> Response {
    let jobs = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT COUNT(*) FILTER (WHERE status = 'ready'),
               COUNT(*) FILTER (WHERE status = 'leased'),
               COUNT(*) FILTER (WHERE status = 'dead')
        FROM judge_jobs
        "#,
    )
    .fetch_one(state.pool())
    .await;
    let workers = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT COUNT(*) FILTER (WHERE last_heartbeat_at >= now() - interval '30 seconds'),
               COUNT(*) FILTER (WHERE last_heartbeat_at < now() - interval '30 seconds')
        FROM judge_workers
        "#,
    )
    .fetch_one(state.pool())
    .await;
    let (jobs, workers) = match (jobs, workers) {
        (Ok(jobs), Ok(workers)) => (jobs, workers),
        (Err(error), _) | (_, Err(error)) => {
            tracing::warn!(%error, "운영 지표 조회 실패");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "alpha_metrics_up 0\n",
            )
                .into_response();
        }
    };
    let body = format!(
        concat!(
            "# TYPE alpha_http_requests_total counter\n",
            "alpha_http_requests_total {}\n",
            "# TYPE alpha_http_server_errors_total counter\n",
            "alpha_http_server_errors_total {}\n",
            "# TYPE alpha_judge_jobs gauge\n",
            "alpha_judge_jobs{{status=\"ready\"}} {}\n",
            "alpha_judge_jobs{{status=\"leased\"}} {}\n",
            "alpha_judge_jobs{{status=\"dead\"}} {}\n",
            "# TYPE alpha_judge_workers gauge\n",
            "alpha_judge_workers{{health=\"healthy\"}} {}\n",
            "alpha_judge_workers{{health=\"stale\"}} {}\n",
            "alpha_metrics_up 1\n"
        ),
        state.runtime().request_count(),
        state.runtime().server_error_count(),
        jobs.0,
        jobs.1,
        jobs.2,
        workers.0,
        workers.1,
    );
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
