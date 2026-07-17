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

async fn authenticated_app(pool: PgPool, handle: &str) -> (axum::Router, String, String) {
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
                .body(Body::from(json!({"handle": handle}).to_string()))
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
    (app, cookie, csrf)
}

async fn post_json(
    app: &axum::Router,
    path: &str,
    cookie: &str,
    csrf: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(path)
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn private_rubric_never_leaks_and_structured_wrong_answer_scores_lower(pool: PgPool) {
    let (app, cookie, csrf) = authenticated_app(pool, "activity-reader").await;
    let detail = app
        .clone()
        .oneshot(
            Request::get("/api/v1/activities/locate-binary-search-bug")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: Value =
        serde_json::from_slice(&to_bytes(detail.into_body(), 32_768).await.unwrap()).unwrap();
    let serialized = detail.to_string();
    assert!(!serialized.contains("evaluator_config"));
    assert!(!serialized.contains("\"expected\""));

    let wrong = post_json(
        &app,
        "/api/v1/activities/locate-binary-search-bug/attempts",
        &cookie,
        &csrf,
        json!({
            "response": {"fields": {
                "line": "8", "category": "오프바이원", "counterexample": "3"
            }},
            "reflection": "구간을 줄이는 줄이 의심스럽다."
        }),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::OK);
    let wrong: Value =
        serde_json::from_slice(&to_bytes(wrong.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(wrong["score"], 66);
    assert_eq!(wrong["passed"], false);

    let correct = post_json(
        &app,
        "/api/v1/activities/locate-binary-search-bug/attempts",
        &cookie,
        &csrf,
        json!({"response": {"fields": {
            "line": "8", "category": "경계 축소 오류", "counterexample": "3"
        }}}),
    )
    .await;
    assert_eq!(correct.status(), StatusCode::OK);
    let correct: Value =
        serde_json::from_slice(&to_bytes(correct.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(correct["score"], 100);
    assert_eq!(correct["mastery_class"], "independent");
}

#[sqlx::test(migrations = "./migrations")]
async fn received_help_is_server_owned_and_prevents_forged_independent_mastery(pool: PgPool) {
    let (app, cookie, csrf) = authenticated_app(pool.clone(), "assisted-reader").await;
    let help = post_json(
        &app,
        "/api/v1/activities/predict-nested-loop-output/assistance/1",
        &cookie,
        &csrf,
        json!({}),
    )
    .await;
    assert_eq!(help.status(), StatusCode::OK);

    let forged = post_json(
        &app,
        "/api/v1/activities/predict-nested-loop-output/attempts",
        &cookie,
        &csrf,
        json!({
            "response": {"answer": "3"},
            "max_assistance_level": 0,
            "mastery_class": "independent"
        }),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let accepted = post_json(
        &app,
        "/api/v1/activities/predict-nested-loop-output/attempts",
        &cookie,
        &csrf,
        json!({"response": {"answer": "3"}}),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted: Value =
        serde_json::from_slice(&to_bytes(accepted.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(accepted["mastery_class"], "assisted");
    assert_eq!(accepted["max_assistance_level"], 1);

    let stored: (i16, String) = sqlx::query_as(
        r#"
        SELECT attempt.max_assistance_level, attempt.mastery_class
        FROM activity_attempts attempt
        JOIN learning_activities activity ON activity.id = attempt.activity_id
        WHERE activity.slug = 'predict-nested-loop-output'
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (1, "assisted".to_owned()));
}

#[sqlx::test(migrations = "./migrations")]
async fn ai_levels_stay_disabled_and_full_explanation_obeys_activity_policy(pool: PgPool) {
    let (app, cookie, csrf) = authenticated_app(pool.clone(), "ai-policy-reader").await;
    let status = app
        .clone()
        .oneshot(
            Request::get("/api/v1/assistance/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status: Value =
        serde_json::from_slice(&to_bytes(status.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["provider"], "disabled");

    for level in 1..=6 {
        let response = post_json(
            &app,
            &format!("/api/v1/activities/predict-nested-loop-output/assistance/{level}"),
            &cookie,
            &csrf,
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let ai = post_json(
        &app,
        "/api/v1/activities/predict-nested-loop-output/assistance/7",
        &cookie,
        &csrf,
        json!({}),
    )
    .await;
    assert_eq!(ai.status(), StatusCode::SERVICE_UNAVAILABLE);

    sqlx::query(
        r#"
        UPDATE activity_progress SET max_assistance_level = 8
        WHERE activity_id = (SELECT id FROM learning_activities WHERE slug = 'predict-nested-loop-output')
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let full = post_json(
        &app,
        "/api/v1/activities/predict-nested-loop-output/assistance/9",
        &cookie,
        &csrf,
        json!({}),
    )
    .await;
    assert_eq!(full.status(), StatusCode::FORBIDDEN);

    let received_level: i16 = sqlx::query_scalar(
        r#"
        SELECT max_assistance_level FROM activity_progress
        WHERE activity_id = (SELECT id FROM learning_activities WHERE slug = 'predict-nested-loop-output')
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(received_level, 8);
}

#[sqlx::test(migrations = "./migrations")]
async fn docs_track_links_official_reading_to_executable_capstone(pool: PgPool) {
    let app = router(AppState::for_test(pool));
    let response = app
        .oneshot(
            Request::get("/api/v1/tracks/docs-builder")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    assert!(
        body["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| {
                resource["url"] == "https://docs.python.org/3/library/urllib.parse.html"
            })
    );
    assert!(
        body["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|activity| { activity["slug"] == "urllib-contract-checkpoint" })
    );
    assert!(
        body["capstones"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capstone| {
                capstone["problem_slug"] == "docs-url-normalizer"
                    && capstone["exercise_kind"] == "capstone"
            })
    );
}
