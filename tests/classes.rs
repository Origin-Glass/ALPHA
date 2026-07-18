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
                .body(Body::from(json!({"version": "2026-07-18"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terms.status(), StatusCode::OK);
    (app, user_id, cookie, csrf)
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 131_072).await.unwrap()).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn invitation_assignment_progress_and_export_keep_real_class_boundaries(pool: PgPool) {
    let (app, instructor_id, instructor_cookie, instructor_csrf) =
        authenticated_app(pool.clone(), "class-instructor").await;
    let (_, learner_id, learner_cookie, learner_csrf) =
        authenticated_app(pool.clone(), "class-learner").await;
    let (_, _outsider_id, outsider_cookie, outsider_csrf) =
        authenticated_app(pool.clone(), "class-outsider").await;
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'INSTRUCTOR')")
        .bind(instructor_id)
        .execute(&pool)
        .await
        .unwrap();

    let organization = app
        .clone()
        .oneshot(
            Request::post("/api/v1/organizations")
                .header(header::COOKIE, &instructor_cookie)
                .header("x-csrf-token", &instructor_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"slug": "integration-school", "name": "통합 검증 학교"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(organization.status(), StatusCode::CREATED);
    let organization_id = json_body(organization).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let classroom = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/organizations/{organization_id}/classes"))
                .header(header::COOKIE, &instructor_cookie)
                .header("x-csrf-token", &instructor_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name": "통합 기초반"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(classroom.status(), StatusCode::CREATED);
    let class_id = json_body(classroom).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let hidden = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_id}"))
                .header(header::COOKIE, &learner_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let invitation = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/classes/{class_id}/invitations"))
                .header(header::COOKIE, &instructor_cookie)
                .header("x-csrf-token", &instructor_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"role": "learner", "expires_in_days": 7}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invitation.status(), StatusCode::CREATED);
    let code = json_body(invitation).await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let accept = |cookie: &str, csrf: &str| {
        Request::post("/api/v1/class-invitations/accept")
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"code": code}).to_string()))
            .unwrap()
    };
    let accepted = app
        .clone()
        .oneshot(accept(&learner_cookie, &learner_csrf))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let replayed = app
        .clone()
        .oneshot(accept(&outsider_cookie, &outsider_csrf))
        .await
        .unwrap();
    assert_eq!(replayed.status(), StatusCode::BAD_REQUEST);

    let activity_attempt = || {
        Request::post("/api/v1/activities/predict-nested-loop-output/attempts")
            .header(header::COOKIE, &learner_cookie)
            .header("x-csrf-token", &learner_csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"response": {"answer": "3"}, "reflection": null}).to_string(),
            ))
            .unwrap()
    };
    let activity_start = app
        .clone()
        .oneshot(
            Request::post("/api/v1/activities/predict-nested-loop-output/start")
                .header(header::COOKIE, &learner_cookie)
                .header("x-csrf-token", &learner_csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(activity_start.status(), StatusCode::OK);
    assert_eq!(
        app.clone()
            .oneshot(activity_attempt())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let assignment = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/classes/{class_id}/assignments"))
                .header(header::COOKIE, &instructor_cookie)
                .header("x-csrf-token", &instructor_csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "title": "읽고 구현하기", "description": "실제 학습 기록을 집계합니다.",
                        "due_at": "2026-12-30T00:00:00Z",
                        "completion_goal_percent": 80,
                        "items": [
                            {"kind": "activity", "slug": "predict-nested-loop-output"},
                            {"kind": "problem", "slug": "alpha-pair-sum"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(assignment.status(), StatusCode::CREATED);

    assert_eq!(
        app.clone()
            .oneshot(activity_attempt())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let submission = || {
        Request::post("/api/v1/submissions")
            .header(header::COOKIE, &learner_cookie)
            .header("x-csrf-token", &learner_csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "problem_slug": "alpha-pair-sum", "language": "python3",
                    "source": "a,b=map(int,input().split());print(a+b)",
                    "idempotency_key": Uuid::now_v7()
                })
                .to_string(),
            ))
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(submission()).await.unwrap().status(),
        StatusCode::CREATED
    );
    let wrong_job = lease_next_job(&pool, "class-worker", StdDuration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    complete_job(
        &pool,
        wrong_job.job_id,
        wrong_job.lease_token,
        Verdict::WrongAnswer,
        0,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        app.clone().oneshot(submission()).await.unwrap().status(),
        StatusCode::CREATED
    );
    let accepted_job = lease_next_job(&pool, "class-worker", StdDuration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    complete_job(
        &pool,
        accepted_job.job_id,
        accepted_job.lease_token,
        Verdict::Accepted,
        100,
        None,
    )
    .await
    .unwrap();

    let learner_detail = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_id}"))
                .header(header::COOKIE, &learner_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let learner_detail = json_body(learner_detail).await;
    assert_eq!(learner_detail["assignments"][0]["completed_items"], 2);
    assert!(
        learner_detail["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["completed"] == true)
    );
    let course_evidence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mastery_events WHERE user_id = $1 AND axis = 'instructor_course' AND mastery_class = 'completed'",
    )
    .bind(learner_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(course_evidence, 1);

    let dashboard = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_id}/instructor-dashboard"))
                .header(header::COOKIE, &instructor_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let dashboard = json_body(dashboard).await;
    assert_eq!(dashboard["assignments"][0]["completion_percent"], 100);
    assert_eq!(dashboard["assignments"][0]["goal_met"], true);
    assert_eq!(dashboard["common_errors"][0]["code"], "WRONG_ANSWER");
    assert_eq!(dashboard["common_errors"][0]["occurrences"], 1);

    sqlx::query("UPDATE users SET display_name = '=HYPERLINK(\"https://bad\")' WHERE id = $1")
        .bind(learner_id)
        .execute(&pool)
        .await
        .unwrap();
    let export = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_id}/export.csv"))
                .header(header::COOKIE, &instructor_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let csv = String::from_utf8(
        to_bytes(export.into_body(), 131_072)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(csv.contains("\"'=HYPERLINK("));
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_events WHERE actor_user_id = $1 AND action = 'class.assignment.created')",
    )
    .bind(instructor_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audited);
}
