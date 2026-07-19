use std::{collections::HashMap, time::Duration};

use alpha::{
    config::Settings,
    http::{AppState, router},
    judge::{Checker, MAX_JOB_LEASE, Verdict, complete_job, lease_next_job},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn authenticated_app(pool: PgPool) -> (axum::Router, Uuid, String, String) {
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
                .body(Body::from(json!({"handle": "judge-learner"}).to_string()))
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
    let user_id = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
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
    (app, user_id, cookie, csrf)
}

#[sqlx::test(migrations = "./migrations")]
async fn submission_idempotency_creates_exactly_one_queue_job(pool: PgPool) {
    let (app, _, cookie, csrf) = authenticated_app(pool.clone()).await;
    let idempotency_key = Uuid::now_v7();
    let request_body = json!({
        "problem_slug": "alpha-pair-sum",
        "language": "python3",
        "source": "a, b = map(int, input().split())\nprint(a + b)\n",
        "idempotency_key": idempotency_key
    })
    .to_string();
    let submit = || {
        Request::post("/api/v1/submissions")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(request_body.clone()))
            .unwrap()
    };
    let first = app.clone().oneshot(submit()).await.unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 16_384).await.unwrap()).unwrap();
    let second = app.oneshot(submit()).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(first["id"], second["id"]);

    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM judge_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn pending_job_budget_backpressures_one_user_then_recovers(pool: PgPool) {
    let (app, _, cookie, csrf) = authenticated_app(pool.clone()).await;
    let submit = || {
        Request::post("/api/v1/submissions")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "problem_slug": "alpha-pair-sum", "language": "python3",
                    "source": "print(sum(map(int,input().split())))",
                    "idempotency_key": Uuid::now_v7()
                })
                .to_string(),
            ))
            .unwrap()
    };
    for _ in 0..4 {
        assert_eq!(
            app.clone().oneshot(submit()).await.unwrap().status(),
            StatusCode::CREATED
        );
    }
    assert_eq!(
        app.clone().oneshot(submit()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    let completed = lease_next_job(&pool, "capacity-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    complete_job(
        &pool,
        completed.job_id,
        completed.lease_token,
        Verdict::WrongAnswer,
        0,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        app.oneshot(submit()).await.unwrap().status(),
        StatusCode::CREATED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn global_queue_budget_rejects_new_work_before_persisting_it(pool: PgPool) {
    let backlog_user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (handle, display_name) VALUES ('queue-backlog', '대기열 사용자') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (problem_id, revision_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, current_revision_id FROM problems WHERE slug = 'alpha-pair-sum'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO submissions (
            id, user_id, problem_id, problem_revision_id, language, source, idempotency_key
        )
        SELECT gen_random_uuid(), $1, $2, $3, 'python3', 'print(0)', gen_random_uuid()
        FROM generate_series(1, 200)
        "#,
    )
    .bind(backlog_user)
    .bind(problem_id)
    .bind(revision_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO judge_jobs (submission_id) SELECT id FROM submissions")
        .execute(&pool)
        .await
        .unwrap();

    let (app, _, cookie, csrf) = authenticated_app(pool.clone()).await;
    let response = app
        .oneshot(
            Request::post("/api/v1/submissions")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "problem_slug": "alpha-pair-sum", "language": "python3",
                        "source": "print(3)", "idempotency_key": Uuid::now_v7()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let submissions: i64 = sqlx::query_scalar("SELECT count(*) FROM submissions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(submissions, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn sse_connection_budget_is_released_when_a_stream_closes(pool: PgPool) {
    let (app, _, cookie, csrf) = authenticated_app(pool).await;
    let created = app
        .clone()
        .oneshot(
            Request::post("/api/v1/submissions")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "problem_slug": "alpha-pair-sum", "language": "python3",
                        "source": "print(3)", "idempotency_key": Uuid::now_v7()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let created: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 16_384).await.unwrap()).unwrap();
    let path = format!(
        "/api/v1/submissions/{}/events",
        created["id"].as_str().unwrap()
    );
    let stream = || {
        Request::get(&path)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap()
    };
    let mut open = Vec::new();
    for _ in 0..4 {
        let response = app.clone().oneshot(stream()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    assert_eq!(
        app.clone().oneshot(stream()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    open.pop();
    assert_eq!(
        app.oneshot(stream()).await.unwrap().status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn sample_run_is_queued_without_becoming_a_formal_submission(pool: PgPool) {
    let (app, _, cookie, csrf) = authenticated_app(pool.clone()).await;
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/runs")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "problem_slug": "alpha-pair-sum",
                        "language": "python3",
                        "source": "a,b=map(int,input().split())\nprint(a+b)\n",
                        "mode": "sample",
                        "idempotency_key": Uuid::now_v7()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(body["run_kind"], "sample");
    let kind: String = sqlx::query_scalar("SELECT run_kind FROM submissions WHERE id = $1")
        .bind(Uuid::parse_str(body["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, "sample");
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_lease_is_recovered_and_stale_worker_cannot_overwrite_verdict(pool: PgPool) {
    let (_, user_id, _, _) = authenticated_app(pool.clone()).await;
    let problem: (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, current_revision_id FROM problems WHERE slug = 'alpha-pair-sum'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let submission_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO submissions (id, user_id, problem_id, problem_revision_id, language, source, idempotency_key)
        VALUES ($1, $2, $3, $4, 'python3', 'print(3)', $5)
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(problem.0)
    .bind(problem.1)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO judge_jobs (submission_id) VALUES ($1)")
        .bind(submission_id)
        .execute(&pool)
        .await
        .unwrap();

    let first = lease_next_job(&pool, "worker-a", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE judge_jobs SET lease_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(first.job_id)
    .execute(&pool)
    .await
    .unwrap();
    let recovered = lease_next_job(&pool, "worker-b", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.job_id, first.job_id);
    assert_eq!(recovered.attempt, 2);

    assert!(
        complete_job(
            &pool,
            first.job_id,
            first.lease_token,
            Verdict::SystemError,
            0,
            None,
        )
        .await
        .is_err()
    );
    complete_job(
        &pool,
        recovered.job_id,
        recovered.lease_token,
        Verdict::Accepted,
        100,
        None,
    )
    .await
    .unwrap();
    let verdict: String = sqlx::query_scalar("SELECT status FROM submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(verdict, "ACCEPTED");
    let progression: (i64, i32, i32) = sqlx::query_as(
        "SELECT xp, mastered_count, independent_mastered_count FROM progression_state WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        progression,
        (100, 1, 1),
        "정식 정답은 문제 보상과 첫 일일 퀘스트만 한 번 반영해야 한다"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn owner_cancellation_invalidates_the_queued_job(pool: PgPool) {
    let (app, _, cookie, csrf) = authenticated_app(pool.clone()).await;
    let created = app
        .clone()
        .oneshot(
            Request::post("/api/v1/submissions")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "problem_slug": "alpha-pair-sum",
                        "language": "python3",
                        "source": "print(sum(map(int,input().split())))",
                        "idempotency_key": Uuid::now_v7()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let created: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 16_384).await.unwrap()).unwrap();
    let submission_id = created["id"].as_str().unwrap();
    let cancelled = app
        .oneshot(
            Request::post(format!("/api/v1/submissions/{submission_id}/cancel"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    let state: (String, String) = sqlx::query_as(
        "SELECT submission.status, job.status FROM submissions submission JOIN judge_jobs job ON job.submission_id = submission.id WHERE submission.id = $1",
    )
    .bind(Uuid::parse_str(submission_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, ("CANCELLED".to_owned(), "done".to_owned()));
}

#[sqlx::test(migrations = "./migrations")]
async fn audited_admin_rejudge_returns_terminal_formal_submission_to_queue(pool: PgPool) {
    let (app, user_id, cookie, csrf) = authenticated_app(pool.clone()).await;
    let problem: (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, current_revision_id FROM problems WHERE slug = 'alpha-pair-sum'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let submission_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO submissions (id, user_id, problem_id, problem_revision_id, language, source, idempotency_key) VALUES ($1,$2,$3,$4,'python3','print(3)',$5)",
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(problem.0)
    .bind(problem.1)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO judge_jobs (submission_id, status) VALUES ($1, 'done')")
        .bind(submission_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE submissions SET status='ACCEPTED', score=100, judged_at=now() WHERE id=$1")
        .bind(submission_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'ADMIN')")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/problems/alpha-pair-sum/rejudge")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"reason": "테스트 데이터 교정 후 전체 재채점"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let state: (String, String) = sqlx::query_as(
        "SELECT submission.status, job.status FROM submissions submission JOIN judge_jobs job ON job.submission_id = submission.id WHERE submission.id = $1",
    )
    .bind(submission_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, ("QUEUED".to_owned(), "ready".to_owned()));
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_events WHERE actor_user_id=$1 AND action='judge.rejudge.requested')",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audited);
    let duplicate = app
        .oneshot(
            Request::post("/api/v1/admin/problems/alpha-pair-sum/rejudge")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"reason": "같은 문제의 중복 재채점 요청 차단"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn checkers_distinguish_formatting_tolerance_and_real_wrong_answers() {
    assert!(Checker::Exact.accepts("3\n", "3\n"));
    assert!(!Checker::Exact.accepts("3", "3\n"));
    assert!(Checker::Whitespace.accepts("1  2\n3", "1 2 3\n"));
    assert!(Checker::Float { tolerance: 1e-6 }.accepts("0.3333334", "0.3333333"));
    assert!(!Checker::Float { tolerance: 1e-9 }.accepts("0.34", "0.33"));
}

#[test]
fn production_lease_covers_the_largest_allowed_problem_budget() {
    let compile_budget = Duration::from_secs(30);
    let maximum_test_budget = Duration::from_millis(200 * (30_000 + 250));
    assert!(MAX_JOB_LEASE >= compile_budget + maximum_test_budget);
}
