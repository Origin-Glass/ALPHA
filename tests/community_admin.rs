use std::collections::HashMap;

use alpha::{
    config::Settings,
    http::{AppState, router},
    judge::{self, load_test_cases},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn authenticated_app(pool: PgPool, handle: &str) -> (axum::Router, Uuid, String, String) {
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
    let body = json_body(login).await;
    let csrf = body["csrf_token"].as_str().unwrap().to_owned();
    let user_id = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
    let terms = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/terms")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"version": "2026-07-18"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terms.status(), StatusCode::OK);
    (app, user_id, cookie, csrf)
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 262_144).await.unwrap()).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn reported_content_is_hidden_only_by_audited_moderation(pool: PgPool) {
    let (app, _author_id, author_cookie, author_csrf) =
        authenticated_app(pool.clone(), "community-author").await;
    let (_, _answerer_id, answerer_cookie, answerer_csrf) =
        authenticated_app(pool.clone(), "community-answerer").await;
    let (_, moderator_id, moderator_cookie, moderator_csrf) =
        authenticated_app(pool.clone(), "comm-mod").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'MODERATOR')")
        .bind(moderator_id)
        .execute(&pool)
        .await
        .unwrap();

    let created = app
        .clone()
        .oneshot(
            Request::post("/api/v1/community")
                .header(header::COOKIE, &author_cookie)
                .header("x-csrf-token", &author_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "kind": "question", "title": "경계 입력 질문",
                        "body": "<img src=x onerror=alert(1)> 빈 입력에서는 무엇을 확인해야 하나요?",
                        "problem_slug": "alpha-pair-sum"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let (_, other_setter_id, other_setter_cookie, other_setter_csrf) =
        authenticated_app(pool.clone(), "other-setter").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'PROBLEM_SETTER')")
        .bind(other_setter_id)
        .execute(&pool)
        .await
        .unwrap();
    let foreign_revision = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/problems/revision-stability/revisions")
                .header(header::COOKIE, &other_setter_cookie)
                .header("x-csrf-token", &other_setter_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    problem_payload("권한 없는 개정", "0\n", "0\n").to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_revision.status(), StatusCode::NOT_FOUND);
    let foreign_rejudge = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/problems/revision-stability/rejudge")
                .header(header::COOKIE, &other_setter_cookie)
                .header("x-csrf-token", &other_setter_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"reason": "다른 출제자의 문제를 재채점할 수 없어야 합니다"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_rejudge.status(), StatusCode::NOT_FOUND);
    let post_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let answer = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/community/{post_id}/answers"))
                .header(header::COOKIE, &answerer_cookie)
                .header("x-csrf-token", &answerer_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"body": "입력 계약과 토큰 수를 먼저 확인하세요."}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status(), StatusCode::CREATED);
    let answer_id = json_body(answer).await["id"].as_str().unwrap().to_owned();
    let accepted = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/v1/community/{post_id}/answers/{answer_id}/accept"
            ))
            .header(header::COOKIE, &author_cookie)
            .header("x-csrf-token", &author_csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);
    let detail = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/community/{post_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail = json_body(detail).await;
    assert_eq!(detail["answers"][0]["accepted"], true);
    assert_eq!(
        detail["post"]["body"],
        "<img src=x onerror=alert(1)> 빈 입력에서는 무엇을 확인해야 하나요?"
    );
    assert!(detail["post"].get("html").is_none());

    let report = || {
        Request::post("/api/v1/community/reports")
            .header(header::COOKIE, &answerer_cookie)
            .header("x-csrf-token", &answerer_csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "target_type": "post", "target_id": post_id,
                    "reason": "solution_leak", "detail": "정답 노출 여부를 확인해 주세요."
                })
                .to_string(),
            ))
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(report()).await.unwrap().status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.clone().oneshot(report()).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let missing_report = app
        .clone()
        .oneshot(
            Request::post("/api/v1/community/reports")
                .header(header::COOKIE, &answerer_cookie)
                .header("x-csrf-token", &answerer_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "target_type": "post", "target_id": Uuid::now_v7(),
                        "reason": "other", "detail": "존재하지 않는 대상"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_report.status(), StatusCode::NOT_FOUND);
    let forbidden = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/community/reports")
                .header(header::COOKIE, &author_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let queue = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/community/reports")
                .header(header::COOKIE, &moderator_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let report_id = json_body(queue).await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resolved = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/v1/admin/community/reports/{report_id}/resolve"
            ))
            .header(header::COOKIE, &moderator_cookie)
            .header("x-csrf-token", &moderator_csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"action": "hide", "note": "운영 정책 위반 확인"}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::NO_CONTENT);
    let hidden = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/community/{post_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_events WHERE actor_user_id = $1 AND action = 'community.report.hidden')",
    )
    .bind(moderator_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audited);
}

fn problem_payload(statement: &str, sample_output: &str, hidden_output: &str) -> Value {
    json!({
        "title": "개정 재현성 문제", "statement": statement,
        "difficulty": 3, "learning_axis": "algorithmic_reasoning", "status": "draft",
        "time_limit_ms": 1000, "memory_limit_mb": 128,
        "checker_kind": "whitespace", "float_tolerance": null,
        "tags": [{"tag": "implementation", "label": "구현"}],
        "test_cases": [
            {"input": "1\n", "expected_output": sample_output, "visibility": "sample", "score_weight": 1, "group_key": "main"},
            {"input": "2\n", "expected_output": hidden_output, "visibility": "hidden", "score_weight": 1, "group_key": "main"}
        ]
    })
}

async fn publish_current_revision(pool: &PgPool, slug: &str) {
    sqlx::query(
        "UPDATE problems SET status = 'published', published_at = now() WHERE slug = $1 AND current_revision_id IS NOT NULL",
    )
    .bind(slug)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn queued_submission_keeps_its_revision_tests_after_problem_update(pool: PgPool) {
    let (app, setter_id, setter_cookie, setter_csrf) =
        authenticated_app(pool.clone(), "problem-setter").await;
    let (_, _ordinary_id, ordinary_cookie, _ordinary_csrf) =
        authenticated_app(pool.clone(), "admin-check").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'PROBLEM_SETTER')")
        .bind(setter_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut create_payload = problem_payload("첫 번째 설명", "2\n", "4\n");
    create_payload["slug"] = json!("revision-stability");
    let created = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/problems")
                .header(header::COOKIE, &setter_cookie)
                .header("x-csrf-token", &setter_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    publish_current_revision(&pool, "revision-stability").await;

    let submission = app
        .clone()
        .oneshot(
            Request::post("/api/v1/submissions")
                .header(header::COOKIE, &setter_cookie)
                .header("x-csrf-token", &setter_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "problem_slug": "revision-stability", "language": "python3",
                        "source": "print(int(input())*2)", "idempotency_key": Uuid::now_v7()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(submission.status(), StatusCode::CREATED);
    let job = judge::lease_next_job(&pool, "revision-worker", std::time::Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();

    let revised = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/problems/revision-stability/revisions")
                .header(header::COOKIE, &setter_cookie)
                .header("x-csrf-token", &setter_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    problem_payload("두 번째 설명", "3\n", "6\n").to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revised.status(), StatusCode::OK);
    publish_current_revision(&pool, "revision-stability").await;
    let old_tests = load_test_cases(&pool, job.problem_revision_id, "formal")
        .await
        .unwrap();
    assert_eq!(old_tests[0].expected_output, "2\n");
    assert_eq!(old_tests[1].expected_output, "4\n");
    let public = app
        .clone()
        .oneshot(
            Request::get("/api/v1/problems/revision-stability")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let public = json_body(public).await;
    assert_eq!(public["problem"]["statement"], "두 번째 설명");
    assert_eq!(public["problem"]["samples"][0]["expected_output"], "3\n");
    assert!(!public.to_string().contains("6\\n"));
    let revision_test_counts: Vec<i64> = sqlx::query_scalar(
        "SELECT COUNT(*) FROM problem_test_cases WHERE problem_id = (SELECT id FROM problems WHERE slug = 'revision-stability') GROUP BY problem_revision_id ORDER BY COUNT(*)",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(revision_test_counts, vec![2, 2]);
    let current_revision: Uuid = sqlx::query_scalar(
        "SELECT current_revision_id FROM problems WHERE slug = 'revision-stability'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let wrong_problem_id: Uuid =
        sqlx::query_scalar("SELECT id FROM problems WHERE slug = 'alpha-pair-sum'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mismatched_test = sqlx::query(
        "INSERT INTO problem_test_cases (problem_id, problem_revision_id, ordinal, input, expected_output, visibility, score_weight, group_key) VALUES ($1,$2,99,'','', 'hidden',1,'integrity')",
    )
    .bind(wrong_problem_id)
    .bind(current_revision)
    .execute(&pool)
    .await;
    assert!(mismatched_test.is_err());

    let forbidden = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/audit")
                .header(header::COOKIE, &ordinary_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'ADMIN')")
        .bind(setter_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO judge_workers (worker_id, protocol_version, image_reference, status) VALUES ('admin-visible-worker', 1, 'alpha-judge@sha256:test', 'ready')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let workers = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/judge/workers")
                .header(header::COOKIE, &setter_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json_body(workers).await["items"][0]["health"], "healthy");
    let audit = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/audit?action=problem.revised")
                .header(header::COOKIE, &setter_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let audit = json_body(audit).await;
    assert_eq!(audit["items"][0]["action"], "problem.revised");
}
