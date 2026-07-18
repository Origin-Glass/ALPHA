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
use uuid::Uuid;

struct Session {
    cookie: String,
    csrf: String,
}

async fn session(app: &axum::Router) -> Session {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle":"project-learner"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("alpha_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    let session = Session {
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    };
    assert_eq!(
        post(
            app,
            &session,
            "/api/v1/auth/terms",
            json!({"version":"2026-07-18","choices":{"terms":true,"privacy":true}})
        )
        .await
        .status(),
        StatusCode::OK
    );
    session
}

fn app(pool: PgPool) -> axum::Router {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    router(AppState::new(pool, settings))
}

async fn post(
    app: &axum::Router,
    session: &Session,
    uri: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(header::COOKIE, &session.cookie)
                .header("x-csrf-token", &session.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 131_072).await.unwrap()).unwrap()
}

fn plan_request(key: Uuid) -> Value {
    json!({
        "rule_version":"project-learning-v1", "idempotency_key":key,
        "target_outcome":"한국어 학습 기록 웹 앱 완성", "weekly_minutes":180,
        "preferred_language":"typescript", "path_mode":"structured",
        "interests":["웹", "학습 기록"], "goals":["독립 구현", "디버깅"],
        "diagnostic_scores":{"algorithmic_reasoning":50,"code_literacy":25,"docs_learning":75,"independent_coding":25}
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn same_input_and_rule_produce_same_auditable_plan(pool: PgPool) {
    let app = app(pool);
    let user = session(&app).await;
    let first = post(
        &app,
        &user,
        "/api/v1/learning/plans",
        plan_request(Uuid::now_v7()),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first = json_body(first).await;
    let second = json_body(
        post(
            &app,
            &user,
            "/api/v1/learning/plans",
            plan_request(Uuid::now_v7()),
        )
        .await,
    )
    .await;
    assert_eq!(first["plan_hash"], second["plan_hash"]);
    assert_eq!(first["reason_codes"], second["reason_codes"]);
    assert_eq!(first["rule_version"], "project-learning-v1");
    assert_eq!(first["provider_used"], false);
    assert_eq!(first["items"][0]["estimated_minutes"], 60);
    assert!(first["revision"].as_i64().unwrap() < second["revision"].as_i64().unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_plan_idempotency_creates_one_revision(pool: PgPool) {
    let app = app(pool.clone());
    let user = session(&app).await;
    let key = Uuid::now_v7();
    let (left, right) = tokio::join!(
        post(&app, &user, "/api/v1/learning/plans", plan_request(key)),
        post(&app, &user, "/api/v1/learning/plans", plan_request(key))
    );
    assert!(matches!(
        left.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    assert!(matches!(
        right.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    let left = json_body(left).await;
    let right = json_body(right).await;
    assert_eq!(left["id"], right["id"]);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM learning_plan_revisions WHERE idempotency_key=$1")
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn rejection_is_recorded_and_not_forced_on_replan(pool: PgPool) {
    let app = app(pool.clone());
    let user = session(&app).await;
    let first = json_body(
        post(
            &app,
            &user,
            "/api/v1/learning/plans",
            plan_request(Uuid::now_v7()),
        )
        .await,
    )
    .await;
    let recommendation = first["recommendation_key"].as_str().unwrap();
    let rejected = post(
        &app,
        &user,
        &format!("/api/v1/learning/recommendations/{recommendation}/reject"),
        json!({"reason":"현재 목표와 맞지 않음","idempotency_key":Uuid::now_v7()}),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::OK);
    let next = json_body(
        post(
            &app,
            &user,
            "/api/v1/learning/plans",
            plan_request(Uuid::now_v7()),
        )
        .await,
    )
    .await;
    assert_ne!(next["recommendation_key"], recommendation);
    let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_recommendation_rejections WHERE reason = '현재 목표와 맞지 않음'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(recorded, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn oversized_idea_is_reduced_to_feasible_owned_choices(pool: PgPool) {
    let app = app(pool);
    let user = session(&app).await;
    let response = post(
        &app,
        &user,
        "/api/v1/projects/ideas",
        json!({
            "title":"모든 기능이 있는 학습 서비스", "motivation":"학습을 기록하고 싶음",
            "target_user":"혼자 공부하는 학생", "intended_outcome":"오늘 기록을 확인",
            "core_feature":"학습 기록", "technology":"typescript", "weekly_minutes":120,
            "requested_features":["로그인","기록","통계","친구","채팅","랭킹","AI 코치","결제"],
            "assistance_policy":"socratic_ai", "idempotency_key":Uuid::now_v7()
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let idea = json_body(response).await;
    assert_eq!(idea["provider_used"], false);
    assert!(idea["features"].as_array().unwrap().len() <= 5);
    assert!(idea["milestones"].as_array().unwrap().len() <= 3);
    assert!((30..=120).contains(&idea["milestones"][0]["estimated_minutes"].as_i64().unwrap()));
    assert_eq!(idea["milestones"][0]["visible_result"], true);
    assert_eq!(idea["scope_reduced"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn mastery_reduces_support_and_repeated_failure_raises_only_one_step(pool: PgPool) {
    let app = app(pool);
    let user = session(&app).await;
    let idea = json_body(post(&app, &user, "/api/v1/projects/ideas", json!({
        "title":"작은 기록 앱","motivation":"기록","target_user":"나","intended_outcome":"기록 보기",
        "core_feature":"기록","technology":"typescript","weekly_minutes":120,
        "requested_features":["기록","목록"],"assistance_policy":"documentation_navigator","idempotency_key":Uuid::now_v7()
    })).await).await;
    let project = json_body(
        post(
            &app,
            &user,
            "/api/v1/projects",
            json!({"idea_id":idea["id"],"idempotency_key":Uuid::now_v7()}),
        )
        .await,
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let milestone_id = project["milestones"][0]["id"].as_str().unwrap();
    let mastery = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"mastery","successful":true,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(mastery["level"], 2);
    let first_failure = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"attempt","successful":false,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(first_failure["level"], 2);
    let repeated = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"attempt","successful":false,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(repeated["level"], 3);
    assert_eq!(repeated["previous_level"], 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn disabled_ai_still_completes_plan_idea_project_and_help(pool: PgPool) {
    let app = app(pool);
    let user = session(&app).await;
    let plan = json_body(
        post(
            &app,
            &user,
            "/api/v1/learning/plans",
            plan_request(Uuid::now_v7()),
        )
        .await,
    )
    .await;
    let idea = json_body(post(&app, &user, "/api/v1/projects/ideas", json!({
        "title":"CLI 할 일","motivation":"자동화","target_user":"나","intended_outcome":"할 일 추가",
        "core_feature":"추가","technology":"rust","weekly_minutes":90,"requested_features":["추가"],
        "assistance_policy":"curated_documentation","idempotency_key":Uuid::now_v7()
    })).await).await;
    let project = json_body(
        post(
            &app,
            &user,
            "/api/v1/projects",
            json!({"idea_id":idea["id"],"idempotency_key":Uuid::now_v7()}),
        )
        .await,
    )
    .await;
    let help = json_body(
        post(
            &app,
            &user,
            &format!(
                "/api/v1/projects/{}/assistance",
                project["id"].as_str().unwrap()
            ),
            json!({"milestone_id":project["milestones"][0]["id"],"skill":"rust"}),
        )
        .await,
    )
    .await;
    assert_eq!(plan["provider_used"], false);
    assert_eq!(idea["provider_used"], false);
    assert_eq!(help["provider_used"], false);
    assert_eq!(help["rule_version"], "project-learning-v1");
    assert!(help["content"].as_str().unwrap().contains("문서"));
}
