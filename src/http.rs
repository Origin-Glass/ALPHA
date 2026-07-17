use axum::{Json, Router, http::StatusCode, routing::get};
use serde_json::{Value, json};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/api/v1/problems", get(crate::problems::list))
        .route("/api/v1/problems/{slug}", get(crate::problems::detail))
        .with_state(state.pool)
}

async fn liveness() -> Json<Value> {
    Json(json!({"service": "alpha-api", "status": "ok"}))
}

async fn readiness(
    axum::extract::State(pool): axum::extract::State<PgPool>,
) -> (StatusCode, Json<Value>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
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
