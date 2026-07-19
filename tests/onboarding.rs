use std::collections::HashMap;

use alpha::{
    config::Settings,
    http::{AppState, router},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn authenticated_app(pool: PgPool) -> (axum::Router, String, String) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool, settings));
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle": "diagnostic-user"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("alpha_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value =
        serde_json::from_slice(&to_bytes(login.into_body(), 16_384).await.unwrap()).unwrap();
    let csrf = body["csrf_token"].as_str().unwrap().to_owned();
    let accepted = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/terms")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"version": "2026-07-18", "choices": {"terms": true, "privacy": true}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    (app, cookie, csrf)
}

#[sqlx::test(migrations = "./migrations")]
async fn server_scored_diagnostic_recommends_weak_axis_and_locks_prerequisite(pool: PgPool) {
    let (app, cookie, csrf) = authenticated_app(pool).await;
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/onboarding/complete")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "answers": [
                            {"question_key": "algorithm-boundary", "selected_option": 1},
                            {"question_key": "code-reading-loop", "selected_option": 0},
                            {"question_key": "docs-contract", "selected_option": 1},
                            {"question_key": "independent-debugging", "selected_option": 0}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32_768).await.unwrap()).unwrap();
    assert_eq!(result["recommended_track"]["slug"], "code-reader");
    assert_eq!(result["scores"]["code_literacy"], 0);

    let path = app
        .oneshot(
            Request::get("/api/v1/learning/path")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(path.status(), StatusCode::OK);
    let path: Value =
        serde_json::from_slice(&to_bytes(path.into_body(), 32_768).await.unwrap()).unwrap();
    assert_eq!(path["track"]["slug"], "code-reader");
    assert_eq!(path["units"][0]["available"], true);
    assert_eq!(path["units"][1]["available"], false);
}
