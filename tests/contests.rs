use std::{collections::HashMap, time::Duration as StdDuration};

use alpha::{
    config::Settings,
    http::{AppState, router},
    judge::{Verdict, complete_job, lease_next_job},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
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
    let body: Value =
        serde_json::from_slice(&to_bytes(login.into_body(), 16_384).await.unwrap()).unwrap();
    let csrf = body["csrf_token"].as_str().unwrap().to_owned();
    let user_id = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
    let terms = app
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
    assert_eq!(terms.status(), StatusCode::OK);
    (app, user_id, cookie, csrf)
}

async fn seed_submission(
    pool: &PgPool,
    contest_id: Uuid,
    user_id: Uuid,
    status: &str,
    score: i16,
    created_at: OffsetDateTime,
) {
    let (problem_id, revision_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, current_revision_id FROM problems WHERE slug = 'alpha-pair-sum'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO submissions (
            user_id, problem_id, problem_revision_id, language, source, idempotency_key,
            status, score, judged_at, created_at, contest_id
        ) VALUES ($1,$2,$3,'python3','print(3)',$4,$5,$6,$7,$7,$8)
        "#,
    )
    .bind(user_id)
    .bind(problem_id)
    .bind(revision_id)
    .bind(Uuid::now_v7())
    .bind(status)
    .bind(score)
    .bind(created_at)
    .bind(contest_id)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn private_contest_stays_hidden_until_the_invite_code_is_verified(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let invite_code = "0123456789abcdefghijklmnopqrstuv";
    sqlx::query(
        "UPDATE contests SET visibility = 'private', join_code_hash = $1, starts_at = $2, freezes_at = NULL, ends_at = $3 WHERE slug = 'alpha-launch-sprint'",
    )
    .bind(Sha256::digest(invite_code.as_bytes()).to_vec())
    .bind(now - Duration::hours(1))
    .bind(now + Duration::hours(1))
    .execute(&pool)
    .await
    .unwrap();
    let (app, user_id, cookie, csrf) = authenticated_app(pool.clone(), "private-contest").await;
    let detail = || {
        Request::get("/api/v1/contests/alpha-launch-sprint")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(detail()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let wrong_code = app
        .clone()
        .oneshot(
            Request::post("/api/v1/contests/alpha-launch-sprint/join")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"join_code": "wrong-code"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_code.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        app.clone().oneshot(detail()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let joined = app
        .clone()
        .oneshot(
            Request::post("/api/v1/contests/alpha-launch-sprint/join")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"join_code": invite_code}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(joined.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.clone().oneshot(detail()).await.unwrap().status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE contest_registrations SET status = 'disqualified' WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let rejoin = app
        .oneshot(
            Request::post("/api/v1/contests/alpha-launch-sprint/join")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"join_code": invite_code}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejoin.status(), StatusCode::FORBIDDEN);
    let status: String =
        sqlx::query_scalar("SELECT status FROM contest_registrations WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "disqualified");
}

#[sqlx::test(migrations = "./migrations")]
async fn private_contest_creation_issues_a_server_generated_192_bit_code(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let (app, manager_id, cookie, csrf) = authenticated_app(pool.clone(), "token-manager").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'CONTEST_MANAGER')")
        .bind(manager_id)
        .execute(&pool)
        .await
        .unwrap();
    let starts_at = (now + Duration::hours(1)).format(&Rfc3339).unwrap();
    let ends_at = (now + Duration::hours(2)).format(&Rfc3339).unwrap();
    let body = |join_code: Option<&str>| {
        json!({
            "slug": "generated-private-contest",
            "title": "서버 발급 초대 대회",
            "description": "예측 가능한 운영자 코드를 받지 않습니다.",
            "visibility": "private",
            "organization_id": null,
            "join_code": join_code,
            "scoring_mode": "icpc",
            "starts_at": starts_at,
            "freezes_at": null,
            "ends_at": ends_at,
            "problem_slugs": ["alpha-pair-sum"]
        })
        .to_string()
    };
    let create = |body: String| {
        Request::post("/api/v1/admin/contests")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(create(body(Some("1234"))))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let created = app.oneshot(create(body(None))).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 16_384).await.unwrap()).unwrap();
    let code = created["join_code"].as_str().unwrap();
    assert_eq!(code.len(), 32);
    assert!(
        code.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT join_code_hash FROM contests WHERE slug = $1")
        .bind("generated-private-contest")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Sha256::digest(code.as_bytes()).to_vec());
}

#[sqlx::test(migrations = "./migrations")]
async fn private_contest_failures_lock_one_account_and_the_whole_contest(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let invite_code = "0123456789abcdefghijklmnopqrstuv";
    let contest_id: Uuid = sqlx::query_scalar(
        "UPDATE contests SET visibility = 'private', join_code_hash = $1, starts_at = $2, freezes_at = NULL, ends_at = $3 WHERE slug = 'alpha-launch-sprint' RETURNING id",
    )
    .bind(Sha256::digest(invite_code.as_bytes()).to_vec())
    .bind(now - Duration::hours(1))
    .bind(now + Duration::hours(1))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (first_app, first_id, first_cookie, first_csrf) =
        authenticated_app(pool.clone(), "failed-join-first").await;
    let request = |cookie: &str, csrf: &str, code: &str| {
        Request::post("/api/v1/contests/alpha-launch-sprint/join")
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"join_code": code}).to_string()))
            .unwrap()
    };
    for _ in 0..5 {
        assert_eq!(
            first_app
                .clone()
                .oneshot(request(&first_cookie, &first_csrf, "wrong"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        first_app
            .oneshot(request(&first_cookie, &first_csrf, invite_code))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let user_budget: (i32, bool) = sqlx::query_as(
        "SELECT failures, locked_until > now() FROM contest_join_user_failures WHERE contest_id = $1 AND user_id = $2",
    )
    .bind(contest_id)
    .bind(first_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(user_budget, (5, true));

    sqlx::query(
        "UPDATE contest_join_global_failures SET failures = 59, window_started_at = now(), locked_until = NULL WHERE contest_id = $1",
    )
    .bind(contest_id)
    .execute(&pool)
    .await
    .unwrap();
    let (second_app, _, second_cookie, second_csrf) =
        authenticated_app(pool.clone(), "failed-join-second").await;
    assert_eq!(
        second_app
            .oneshot(request(&second_cookie, &second_csrf, "wrong"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let (third_app, _, third_cookie, third_csrf) =
        authenticated_app(pool.clone(), "failed-join-third").await;
    assert_eq!(
        third_app
            .oneshot(request(&third_cookie, &third_csrf, invite_code))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let global_budget: (i32, bool) = sqlx::query_as(
        "SELECT failures, locked_until > now() FROM contest_join_global_failures WHERE contest_id = $1",
    )
    .bind(contest_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(global_budget, (60, true));
}

#[sqlx::test(migrations = "./migrations")]
async fn icpc_scoreboard_applies_wrong_penalty_and_hides_post_freeze_results(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let start = now - Duration::hours(1);
    let freeze = now + Duration::minutes(30);
    let end = now + Duration::hours(1);
    let contest_id: Uuid = sqlx::query_scalar(
        r#"
        UPDATE contests SET starts_at = $1, freezes_at = $2, ends_at = $3
        WHERE slug = 'alpha-launch-sprint' RETURNING id
        "#,
    )
    .bind(start)
    .bind(freeze)
    .bind(end)
    .fetch_one(&pool)
    .await
    .unwrap();
    let first: Uuid = sqlx::query_scalar(
        "INSERT INTO users (handle, display_name) VALUES ('contest-first', '첫 참가자') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second: Uuid = sqlx::query_scalar(
        "INSERT INTO users (handle, display_name) VALUES ('contest-second', '둘째 참가자') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for user_id in [first, second] {
        sqlx::query("INSERT INTO contest_registrations (contest_id, user_id) VALUES ($1, $2)")
            .bind(contest_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
    }
    seed_submission(
        &pool,
        contest_id,
        first,
        "WRONG_ANSWER",
        0,
        now - Duration::minutes(50),
    )
    .await;
    seed_submission(
        &pool,
        contest_id,
        first,
        "ACCEPTED",
        100,
        now - Duration::minutes(30),
    )
    .await;
    seed_submission(
        &pool,
        contest_id,
        second,
        "ACCEPTED",
        100,
        now - Duration::minutes(15),
    )
    .await;

    let app = router(AppState::for_test(pool.clone()));
    let live = app
        .clone()
        .oneshot(
            Request::get("/api/v1/contests/alpha-launch-sprint/scoreboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let live: Value =
        serde_json::from_slice(&to_bytes(live.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(live["entries"][0]["handle"], "contest-second");
    assert_eq!(live["entries"][0]["penalty"], 45);
    assert_eq!(live["entries"][1]["penalty"], 50);

    let frozen_at = now - Duration::minutes(20);
    sqlx::query("UPDATE contests SET freezes_at = $2 WHERE id = $1")
        .bind(contest_id)
        .bind(frozen_at)
        .execute(&pool)
        .await
        .unwrap();
    let frozen = app
        .oneshot(
            Request::get("/api/v1/contests/alpha-launch-sprint/scoreboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let frozen: Value =
        serde_json::from_slice(&to_bytes(frozen.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(frozen["frozen"], true);
    let second_entry = frozen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["handle"] == "contest-second")
        .unwrap();
    assert_eq!(second_entry["solved"], 0);
    let second_cell = frozen["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["user_id"] == second.to_string() && cell["problem_label"] == "A")
        .unwrap();
    assert_eq!(second_cell["frozen_attempts"], 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_registered_participant_can_attach_submission_to_running_contest(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "UPDATE contests SET starts_at = $1, freezes_at = NULL, ends_at = $2 WHERE slug = 'alpha-launch-sprint'",
    )
    .bind(now - Duration::hours(1))
    .bind(now + Duration::hours(1))
    .execute(&pool)
    .await
    .unwrap();
    let (app, user_id, cookie, csrf) = authenticated_app(pool.clone(), "contest-submit").await;
    let submission = |key: Uuid| {
        Request::post("/api/v1/submissions")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "problem_slug": "alpha-pair-sum", "language": "python3",
                    "source": "print(sum(map(int,input().split())))",
                    "idempotency_key": key, "contest_slug": "alpha-launch-sprint"
                })
                .to_string(),
            ))
            .unwrap()
    };
    let blocked = app
        .clone()
        .oneshot(submission(Uuid::now_v7()))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    let joined = app
        .clone()
        .oneshot(
            Request::post("/api/v1/contests/alpha-launch-sprint/join")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(joined.status(), StatusCode::NO_CONTENT);
    let detail = app
        .clone()
        .oneshot(
            Request::get("/api/v1/contests/alpha-launch-sprint")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail: Value =
        serde_json::from_slice(&to_bytes(detail.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(detail["registered"], true);
    assert!(
        detail["contest"]["starts_at"]
            .as_str()
            .is_some_and(|value| value.ends_with('Z'))
    );
    let accepted = app.oneshot(submission(Uuid::now_v7())).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::CREATED);
    let attached: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM submissions WHERE contest_id IS NOT NULL)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(attached);
    let job = lease_next_job(&pool, "contest-worker", StdDuration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    complete_job(
        &pool,
        job.job_id,
        job.lease_token,
        Verdict::Accepted,
        100,
        None,
    )
    .await
    .unwrap();
    let progression: (i64, i32) = sqlx::query_as(
        "SELECT xp, independent_mastered_count FROM progression_state WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(progression, (120, 0));
    let verified: String = sqlx::query_scalar(
        "SELECT mastery_class FROM mastery_events WHERE user_id = $1 AND axis = 'contest'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(verified, "contest_verified");
}

#[sqlx::test(migrations = "./migrations")]
async fn contest_rating_finalization_is_audited_and_idempotent(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let contest_id: Uuid = sqlx::query_scalar(
        r#"
        UPDATE contests SET starts_at = $1, freezes_at = NULL, ends_at = $2
        WHERE slug = 'alpha-launch-sprint' RETURNING id
        "#,
    )
    .bind(now - Duration::hours(2))
    .bind(now - Duration::minutes(1))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (app, manager_id, cookie, csrf) = authenticated_app(pool.clone(), "contest-manager").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'CONTEST_MANAGER')")
        .bind(manager_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO contest_registrations (contest_id, user_id) VALUES ($1, $2)")
        .bind(contest_id)
        .bind(manager_id)
        .execute(&pool)
        .await
        .unwrap();
    let finalize = || {
        Request::post("/api/v1/admin/contests/alpha-launch-sprint/finalize")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap()
    };
    let first = app.clone().oneshot(finalize()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(first["finalized"], 1);
    let second = app.oneshot(finalize()).await.unwrap();
    let second: Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(second["finalized"], 0);
    let history: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contest_rating_history")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(history, 1);
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_events WHERE action = 'contest.rating.finalized' AND actor_user_id = $1)",
    )
    .bind(manager_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audited);
}
