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
    user_id: Uuid,
    cookie: String,
    csrf: String,
}

async fn login(app: &axum::Router, handle: &str) -> Session {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle": handle}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = response
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
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    Session {
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    }
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

async fn grant(pool: &PgPool, session: &Session, role: &str) {
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, $2)")
        .bind(session.user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

fn draft_payload() -> Value {
    json!({
        "slug": "rights-gated-problem", "title": "권리 검토 문제", "statement": "두 수를 더하세요.",
        "difficulty": 1, "learning_axis": "algorithmic_reasoning", "status": "draft",
        "time_limit_ms": 1000, "memory_limit_mb": 128, "checker_kind": "exact", "float_tolerance": null,
        "tags": [{"tag": "math", "label": "수학"}],
        "test_cases": [
            {"input": "1 2", "expected_output": "3", "visibility": "sample", "score_weight": 1, "group_key": "main"},
            {"input": "2 3", "expected_output": "5", "visibility": "hidden", "score_weight": 1, "group_key": "main"}
        ]
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn publication_requires_usable_rights_and_two_non_author_reviewers(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, "rights-author").await;
    let content_reviewer = login(&app, "content-reviewer").await;
    let rights_reviewer = login(&app, "rights-reviewer").await;
    grant(&pool, &author, "CONTENT_CREATOR").await;
    grant(&pool, &content_reviewer, "CONTENT_REVIEWER").await;
    grant(&pool, &rights_reviewer, "RIGHTS_REVIEWER").await;

    assert_eq!(
        post(&app, &author, "/api/v1/admin/problems", draft_payload())
            .await
            .status(),
        StatusCode::CREATED
    );

    let publish_uri = "/api/v1/governance/problems/rights-gated-problem/publish";
    assert_eq!(
        post(&app, &author, publish_uri, json!({})).await.status(),
        StatusCode::CONFLICT
    );

    let rights_uri = "/api/v1/governance/problems/rights-gated-problem/rights";
    assert_eq!(
        post(
            &app,
            &author,
            rights_uri,
            json!({
                "basis": "original", "evidence": "작성자 원본 제작 기록",
                "commercial_use_allowed": false, "redistribution_allowed": true
            })
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(&app, &author, publish_uri, json!({})).await.status(),
        StatusCode::CONFLICT
    );

    assert_eq!(
        post(
            &app,
            &author,
            "/api/v1/governance/problems/rights-gated-problem/reviews/content",
            json!({"note": "검토 완료"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &app,
            &author,
            "/api/v1/governance/problems/rights-gated-problem/reviews/rights",
            json!({"note": "검토 완료"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        post(
            &app,
            &author,
            rights_uri,
            json!({
                "basis": "original", "evidence": "작성자 원본 제작 기록",
                "commercial_use_allowed": true, "redistribution_allowed": true
            })
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &content_reviewer,
            "/api/v1/governance/problems/rights-gated-problem/reviews/content",
            json!({"note": "문제와 정답 검토 완료"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &content_reviewer,
            "/api/v1/governance/problems/rights-gated-problem/reviews/rights",
            json!({"note": "겸임 시도"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &app,
            &rights_reviewer,
            "/api/v1/governance/problems/rights-gated-problem/reviews/rights",
            json!({"note": "상업·재배포 근거 확인"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(&app, &author, publish_uri, json!({})).await.status(),
        StatusCode::OK
    );

    let public = app
        .clone()
        .oneshot(
            Request::get("/api/v1/problems/rights-gated-problem")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    let published_and_audited: (String, bool) = sqlx::query_as(
        "SELECT problem.status, EXISTS(SELECT 1 FROM audit_events WHERE action = 'content.published' AND target_id = problem.id::text) FROM problems problem WHERE slug = 'rights-gated-problem'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(published_and_audited, ("published".to_owned(), true));
}
