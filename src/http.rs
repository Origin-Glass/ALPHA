use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

pub fn router() -> Router {
    Router::new().route("/health/live", get(liveness))
}

async fn liveness() -> Json<Value> {
    Json(json!({"service": "alpha-api", "status": "ok"}))
}
