use axum::{body::to_bytes, http::Request};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn liveness_reports_the_running_api() {
    let response = alpha::http::router()
        .oneshot(
            Request::get("/health/live")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"service": "alpha-api", "status": "ok"})
    );
}
