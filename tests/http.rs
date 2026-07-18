use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;

use alpha::config::Settings;

#[tokio::test]
async fn liveness_reports_the_running_api() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://alpha:alpha@127.0.0.1:1/alpha")
        .unwrap();
    let response = alpha::http::router(alpha::http::AppState::for_test(pool))
        .oneshot(
            Request::get("/health/live")
                .header("x-request-id", "test-trace-123")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-request-id"], "test-trace-123");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"service": "alpha-api", "status": "ok"})
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn write_rate_limit_rejects_rotating_fake_sessions_without_blocking_reads(pool: PgPool) {
    let app = alpha::http::router(alpha::http::AppState::for_test(pool));
    for index in 0..60 {
        let response = app
            .clone()
            .oneshot(
                Request::post("/health/live")
                    .header("cookie", format!("alpha_session={index:043}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
    let limited = app
        .clone()
        .oneshot(
            Request::post("/health/live")
                .header("cookie", format!("alpha_session={:043}", 61))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "60");

    let read = app
        .oneshot(
            Request::get("/health/live")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn metrics_report_http_and_worker_health(pool: PgPool) {
    sqlx::query(
        "INSERT INTO judge_workers (worker_id, protocol_version, image_reference, status) VALUES ('metrics-worker', 1, 'alpha-judge@sha256:metrics', 'ready')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = alpha::http::router(alpha::http::AppState::for_test(pool));
    app.clone()
        .oneshot(
            Request::get("/health/live")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let response = app
        .oneshot(
            Request::get("/metrics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 16_384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("alpha_http_requests_total 2"));
    assert!(body.contains("alpha_judge_workers{health=\"healthy\"} 1"));
    assert!(body.contains("alpha_metrics_up 1"));
}

#[sqlx::test(migrations = "./migrations")]
async fn authenticated_users_behind_one_school_ip_keep_independent_write_budgets(pool: PgPool) {
    let settings = Settings::from_pairs(std::collections::HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = alpha::http::router(alpha::http::AppState::new(pool, settings));
    let mut cookies = Vec::new();
    for handle in ["school-learner-a", "school-learner-b"] {
        let login = app
            .clone()
            .oneshot(
                Request::post("/api/v1/auth/test-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"handle": handle}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        cookies.push(
            login
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .find(|value| value.starts_with("alpha_session="))
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned(),
        );
    }

    for cookie in &cookies {
        for _ in 0..60 {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/health/live")
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }
    let limited = app
        .oneshot(
            Request::post("/health/live")
                .header(header::COOKIE, &cookies[0])
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
}
