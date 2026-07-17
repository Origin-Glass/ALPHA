use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use alpha::{
    config::Settings,
    http::AppState,
    metadata::{refresh_solved_ac_at, tier_token},
};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn publish_with_initial_revision(pool: &PgPool, problem_id: Uuid) {
    let revision_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO problem_revisions (
            problem_id, version, statement_ko, time_limit_ms, memory_limit_mb,
            checker_kind, content_hash
        )
        SELECT id, 1, statement_ko, time_limit_ms, memory_limit_mb,
               checker_kind, digest(statement_ko, 'sha256')
        FROM problems WHERE id = $1
        RETURNING id
        "#,
    )
    .bind(problem_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE problems SET status = 'published', published_at = now(), current_revision_id = $2 WHERE id = $1",
    )
    .bind(problem_id)
    .bind(revision_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn solved_fixture(State(calls): State<Arc<AtomicUsize>>) -> Response {
    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
        Json(json!({
            "problemId": 1000,
            "level": 0,
            "tags": [{
                "key": "implementation",
                "displayNames": [
                    {"language": "en", "name": "Implementation", "short": "impl"},
                    {"language": "ko", "name": "구현", "short": "구현"}
                ]
            }]
        }))
        .into_response()
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn solved_ac_failure_preserves_last_good_unrated_metadata(pool: PgPool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = Router::new()
        .route("/problem/show", get(solved_fixture))
        .with_state(calls);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/problem/show", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    refresh_solved_ac_at(&pool, &client, "1000", &endpoint)
        .await
        .unwrap();
    assert!(
        refresh_solved_ac_at(&pool, &client, "1000", &endpoint)
            .await
            .is_err()
    );

    let cached: (Option<i16>, Option<String>, Value, String, Option<String>) = sqlx::query_as(
        r#"
        SELECT raw_level, normalized_tier, normalized_tags, state, last_error
        FROM external_problem_metadata_cache
        WHERE provider = 'solved_ac' AND external_problem_id = '1000'
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cached.0, Some(0));
    assert_eq!(cached.1.as_deref(), Some("unrated"));
    assert_eq!(cached.2[0]["label"], "구현");
    assert_eq!(cached.3, "stale");
    assert!(cached.4.unwrap().contains("403"));
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn catalog_combines_keyword_tag_level_and_axis_filters(pool: PgPool) {
    for (slug, title, level, axis, tag) in [
        (
            "binary-boundary",
            "이분 탐색 경계",
            11_i16,
            "algorithmic_reasoning",
            "binary_search",
        ),
        (
            "binary-reading",
            "이분 탐색 코드 읽기",
            12_i16,
            "code_literacy",
            "binary_search",
        ),
        (
            "graph-route",
            "그래프 경로",
            11_i16,
            "algorithmic_reasoning",
            "graphs",
        ),
    ] {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO problems (
                id, slug, title_ko, statement_ko, difficulty, learning_axis,
                status, source_kind, time_limit_ms, memory_limit_mb
            ) VALUES ($1, $2, $3, 'ALPHA 작성 문제', $4, $5, 'draft', 'original', 1000, 256)
            "#,
        )
        .bind(id)
        .bind(slug)
        .bind(title)
        .bind(level)
        .bind(axis)
        .execute(&pool)
        .await
        .unwrap();
        publish_with_initial_revision(&pool, id).await;
        sqlx::query("INSERT INTO problem_tags (problem_id, tag, label_ko) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(tag)
            .bind(if tag == "binary_search" {
                "이분 탐색"
            } else {
                "그래프"
            })
            .execute(&pool)
            .await
            .unwrap();
    }

    let response = alpha::http::router(AppState::for_test(pool))
        .oneshot(
            Request::get("/api/v1/problems?q=%EC%9D%B4%EB%B6%84&tag=binary_search&min_level=11&max_level=11&axis=algorithmic_reasoning")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["slug"], "binary-boundary");
    assert_eq!(body["items"][0]["tags"][0]["label"], "이분 탐색");
}

#[test]
fn solved_ac_tier_boundaries_never_turn_unrated_into_bronze() {
    assert_eq!(tier_token(0), "unrated");
    assert_eq!(tier_token(1), "bronze-v");
    assert_eq!(tier_token(5), "bronze-i");
    assert_eq!(tier_token(6), "silver-v");
    assert_eq!(tier_token(30), "ruby-i");
    assert_eq!(tier_token(31), "special-31");
}

#[sqlx::test(migrations = "./migrations")]
async fn ordinary_user_cannot_trigger_provider_refresh(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = alpha::http::router(AppState::new(pool.clone(), settings));
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle": "metadata-user"}).to_string()))
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
    let login_body: Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let response = app
        .oneshot(
            Request::post("/api/v1/admin/metadata/solved-ac/999999/refresh")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", login_body["csrf_token"].as_str().unwrap())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let cache_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM external_problem_metadata_cache WHERE external_problem_id = '999999')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!cache_exists);
}
