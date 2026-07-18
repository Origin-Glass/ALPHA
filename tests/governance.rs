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

fn policy_consent() -> Value {
    json!({
        "version": "2026-07-18",
        "choices": {"terms": true, "privacy": true}
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
    grant(&pool, &author, "CONTENT_REVIEWER").await;
    grant(&pool, &author, "RIGHTS_REVIEWER").await;
    grant(&pool, &content_reviewer, "CONTENT_REVIEWER").await;
    grant(&pool, &content_reviewer, "RIGHTS_REVIEWER").await;
    grant(&pool, &rights_reviewer, "RIGHTS_REVIEWER").await;
    for session in [&author, &content_reviewer, &rights_reviewer] {
        assert_eq!(
            post(&app, session, "/api/v1/policies/consents", policy_consent())
                .await
                .status(),
            StatusCode::OK
        );
    }

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
            "/api/v1/governance/problems/rights-gated-problem/reviews/rights",
            json!({"note": "권리부터 검토"})
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
            json!({"note": "동일인 겸임 시도"})
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

#[sqlx::test(migrations = "./migrations")]
async fn revision_author_cannot_approve_own_revision(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let problem_author = login(&app, "problem-author").await;
    let revision_author = login(&app, "revision-author").await;
    grant(&pool, &problem_author, "CONTENT_CREATOR").await;
    grant(&pool, &revision_author, "ADMIN").await;
    for session in [&problem_author, &revision_author] {
        assert_eq!(
            post(&app, session, "/api/v1/policies/consents", policy_consent(),)
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        post(
            &app,
            &problem_author,
            "/api/v1/admin/problems",
            draft_payload(),
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let mut revision = draft_payload();
    revision.as_object_mut().unwrap().remove("slug");
    revision["statement"] = json!("관리자가 작성한 새 리비전입니다.");
    assert_eq!(
        post(
            &app,
            &revision_author,
            "/api/v1/admin/problems/rights-gated-problem/revisions",
            revision,
        )
        .await
        .status(),
        StatusCode::OK
    );

    let rights_uri = "/api/v1/governance/problems/rights-gated-problem/rights";
    let rights = json!({
        "basis": "original", "evidence": "원 문제 작성자가 보관한 제작 이력",
        "commercial_use_allowed": true, "redistribution_allowed": true
    });
    assert_eq!(
        post(&app, &problem_author, rights_uri, rights.clone())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &revision_author,
            "/api/v1/governance/problems/rights-gated-problem/reviews/content",
            json!({"note": "자기 리비전 사람 승인 시도"}),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &problem_author, rights_uri, rights)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &revision_author,
            "/api/v1/governance/problems/rights-gated-problem/reviews/rights",
            json!({"note": "자기 리비전 권리 승인 시도"}),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn required_policy_consent_and_data_requests_are_user_scoped(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let owner = login(&app, "data-owner").await;
    let other = login(&app, "data-other").await;
    grant(&pool, &owner, "CONTENT_CREATOR").await;

    let mut payload = draft_payload();
    payload["slug"] = json!("policy-gated-problem");
    assert_eq!(
        post(&app, &owner, "/api/v1/admin/problems", payload)
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        post(
            &app,
            &owner,
            "/api/v1/governance/problems/policy-gated-problem/rights",
            json!({
                "basis": "original", "evidence": "원본 제작 이력",
                "commercial_use_allowed": true, "redistribution_allowed": true
            })
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        post(
            &app,
            &owner,
            "/api/v1/policies/consents",
            json!({"version": "2026-07-18"}),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    for session in [&owner, &other] {
        assert_eq!(
            post(&app, session, "/api/v1/policies/consents", policy_consent())
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        post(
            &app,
            &owner,
            "/api/v1/governance/problems/policy-gated-problem/rights",
            json!({
                "basis": "original", "evidence": "원본 제작 이력",
                "commercial_use_allowed": true, "redistribution_allowed": true
            })
        )
        .await
        .status(),
        StatusCode::OK
    );

    let created = post(
        &app,
        &owner,
        "/api/v1/data-requests",
        json!({"kind": "export"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 16_384).await.unwrap()).unwrap();
    let request_id = created["id"].as_str().unwrap();

    let foreign_read = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/data-requests/{request_id}"))
                .header(header::COOKIE, &other.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_read.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        post(
            &app,
            &other,
            &format!("/api/v1/data-requests/{request_id}/cancel"),
            json!({})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let policy_response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/policies")
                .header(header::COOKIE, &owner.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(policy_response.status(), StatusCode::OK);
    let policy_body: Value =
        serde_json::from_slice(&to_bytes(policy_response.into_body(), 32_768).await.unwrap())
            .unwrap();
    let policy = &policy_body["items"][0];
    assert!(policy["body"].as_str().unwrap().contains("이용약관"));
    assert!(
        policy["body"]
            .as_str()
            .unwrap()
            .contains("개인정보 처리방침")
    );
    assert_eq!(
        policy["consent"]["choices"],
        json!({"terms": true, "privacy": true})
    );
    assert_eq!(policy["consent"]["body"], policy["body"]);

    let stored: (String, String, Value) = sqlx::query_as(
        "SELECT policy_title_ko, policy_body_ko, choices FROM policy_consents WHERE user_id = $1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.0, policy["title"]);
    assert_eq!(stored.1, policy["body"]);
    assert_eq!(stored.2, json!({"terms": true, "privacy": true}));
    let audit_choices: Value = sqlx::query_scalar(
        "SELECT metadata->'choices' FROM audit_events WHERE actor_user_id = $1 AND action = 'policy.consented' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_choices, stored.2);
}
