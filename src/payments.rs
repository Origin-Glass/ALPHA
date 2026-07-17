use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::http::AppState;

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    #[serde(rename = "plan")]
    _plan: String,
}

pub async fn create_checkout(
    State(state): State<AppState>,
    Json(_request): Json<CheckoutRequest>,
) -> Response {
    if !state.settings().payments_enabled || state.settings().payment_provider == "disabled" {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "code": "payments_disabled",
                    "message": "결제 기능은 현재 사용할 수 없습니다"
                }
            })),
        )
            .into_response();
    }

    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": {
                "code": "payment_provider_not_implemented",
                "message": "승인된 결제 제공자가 없습니다"
            }
        })),
    )
        .into_response()
}
