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
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
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
async fn get(app: &axum::Router, user: &Session, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
                .header(header::COOKIE, &user.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
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

fn idea_request(key: Uuid, features: Vec<&str>, policy: &str) -> Value {
    json!({
        "title":"작은 기록 앱","motivation":"기록","target_user":"나","intended_outcome":"기록 보기",
        "core_feature":"기록","technology":"typescript","weekly_minutes":120,"requested_features":features,
        "assistance_policy":policy,"skill_level":"beginner","runtime":"browser","infrastructure":"local_only",
        "source_kind":"original","repository_url":null,"repository_revision":null,"ownership_basis":null,"license_identifier":null,
        "idempotency_key":key
    })
}

async fn confirm(app: &axum::Router, user: &Session, idea: &Value) {
    let response=post(app,user,&format!("/api/v1/projects/ideas/{}/confirm",idea["id"].as_str().unwrap()),json!({
        "scoped_features":idea["features"],"milestones":idea["milestones"],"idempotency_key":Uuid::now_v7()
    })).await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn activity_attempt(pool: &PgPool, user: Uuid, passed: bool, class: Option<&str>) -> Uuid {
    let activity: Uuid =
        sqlx::query_scalar("SELECT id FROM learning_activities ORDER BY id LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar("INSERT INTO activity_attempts (user_id,activity_id,response,score,passed,max_assistance_level,mastery_class,elapsed_seconds) VALUES ($1,$2,'{}',$3,$4,$5,$6,60) RETURNING id")
        .bind(user).bind(activity).bind(if passed{100_i16}else{0_i16}).bind(passed).bind(if class==Some("independent"){0_i16}else{1_i16}).bind(class).fetch_one(pool).await.unwrap()
}

async fn project(app: &axum::Router, user: &Session, policy: &str) -> Value {
    let idea = json_body(
        post(
            app,
            user,
            "/api/v1/projects/ideas",
            idea_request(Uuid::now_v7(), vec!["기록", "목록"], policy),
        )
        .await,
    )
    .await;
    confirm(app, user, &idea).await;
    json_body(
        post(
            app,
            user,
            "/api/v1/projects",
            json!({"idea_id":idea["id"],"idempotency_key":Uuid::now_v7()}),
        )
        .await,
    )
    .await
}

#[sqlx::test(migrations = "./migrations")]
async fn same_input_and_rule_produce_same_auditable_plan(pool: PgPool) {
    let app = app(pool.clone());
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
    let rejection_key = Uuid::now_v7();
    let rejected = post(
        &app,
        &user,
        &format!("/api/v1/learning/recommendations/{recommendation}/reject"),
        json!({"reason":"현재 목표와 맞지 않음","idempotency_key":rejection_key}),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::OK);
    assert_eq!(
        post(
            &app,
            &user,
            &format!("/api/v1/learning/recommendations/{recommendation}/reject"),
            json!({"reason":"현재 목표와 맞지 않음","idempotency_key":rejection_key})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &user,
            &format!("/api/v1/learning/recommendations/{recommendation}/reject"),
            json!({"reason":"다른 사유","idempotency_key":rejection_key})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
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
        idea_request(
            Uuid::now_v7(),
            vec![
                "로그인",
                "기록",
                "통계",
                "친구",
                "채팅",
                "랭킹",
                "AI 코치",
                "결제",
            ],
            "socratic_ai",
        ),
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
    assert!(idea["feasibility_reasons"].as_array().unwrap().len() >= 4);
    assert!(!idea["excluded_features"].as_array().unwrap().is_empty());
    assert_eq!(idea["confirmation_required"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn mastery_reduces_support_and_repeated_failure_raises_only_one_step(pool: PgPool) {
    let app = app(pool.clone());
    let user = session(&app).await;
    let idea = json_body(
        post(
            &app,
            &user,
            "/api/v1/projects/ideas",
            idea_request(
                Uuid::now_v7(),
                vec!["기록", "목록"],
                "documentation_navigator",
            ),
        )
        .await,
    )
    .await;
    confirm(&app, &user, &idea).await;
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
    let independent = activity_attempt(&pool, user.user_id, true, Some("independent")).await;
    let mastery = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"mastery","successful":true,"source_attempt_id":independent,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(mastery["level"], 4);
    let failed1 = activity_attempt(&pool, user.user_id, false, None).await;
    let first_failure = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":failed1,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(first_failure["level"], 4);
    let failed2 = activity_attempt(&pool, user.user_id, false, None).await;
    let repeated = json_body(post(&app, &user, &format!("/api/v1/projects/{project_id}/assistance/evidence"),
        json!({"milestone_id":milestone_id,"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":failed2,"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(repeated["level"], 5);
    assert_eq!(repeated["previous_level"], 4);
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
    let idea = json_body(
        post(
            &app,
            &user,
            "/api/v1/projects/ideas",
            idea_request(Uuid::now_v7(), vec!["기록"], "curated_documentation"),
        )
        .await,
    )
    .await;
    confirm(&app, &user, &idea).await;
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
            json!({"milestone_id":project["milestones"][0]["id"],"skill":"typescript","idempotency_key":Uuid::now_v7()}),
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

#[sqlx::test(migrations = "./migrations")]
async fn assisted_or_missing_receipt_cannot_reduce_support_and_success_resets_failures(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let user = session(&app).await;
    let project = project(&app, &user, "documentation_navigator").await;
    let uri = format!(
        "/api/v1/projects/{}/assistance/evidence",
        project["id"].as_str().unwrap()
    );
    let milestone = &project["milestones"][0]["id"];
    let assisted = activity_attempt(&pool, user.user_id, true, Some("assisted")).await;
    assert_eq!(post(&app,&user,&uri,json!({"milestone_id":milestone,"skill":"typescript","kind":"mastery","successful":true,"source_attempt_id":assisted,"idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::BAD_REQUEST);
    assert_eq!(post(&app,&user,&uri,json!({"milestone_id":milestone,"skill":"typescript","kind":"mastery","successful":true,"source_attempt_id":Uuid::now_v7(),"idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::BAD_REQUEST);
    let failed1 = activity_attempt(&pool, user.user_id, false, None).await;
    assert_eq!(json_body(post(&app,&user,&uri,json!({"milestone_id":milestone,"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":failed1,"idempotency_key":Uuid::now_v7()})).await).await["level"],5);
    let success = activity_attempt(&pool, user.user_id, true, Some("assisted")).await;
    assert_eq!(json_body(post(&app,&user,&uri,json!({"milestone_id":milestone,"skill":"typescript","kind":"attempt","successful":true,"source_attempt_id":success,"idempotency_key":Uuid::now_v7()})).await).await["level"],5);
    for expected in [5, 6] {
        let failed = activity_attempt(&pool, user.user_id, false, None).await;
        let body=json_body(post(&app,&user,&uri,json!({"milestone_id":milestone,"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":failed,"idempotency_key":Uuid::now_v7()})).await).await;
        assert_eq!(body["level"], expected);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_help_replay_converges_and_cross_kind_key_conflicts(pool: PgPool) {
    let app = app(pool.clone());
    let user = session(&app).await;
    let cheat_project = project(&app, &user, "cheat_sheet_only").await;
    let uri = format!(
        "/api/v1/projects/{}/assistance",
        cheat_project["id"].as_str().unwrap()
    );
    let key = Uuid::now_v7();
    let payload = json!({"milestone_id":cheat_project["milestones"][0]["id"],"skill":"typescript","idempotency_key":key});
    let (left, right) = tokio::join!(
        post(&app, &user, &uri, payload.clone()),
        post(&app, &user, &uri, payload)
    );
    assert_eq!(left.status(), StatusCode::OK);
    assert_eq!(right.status(), StatusCode::OK);
    let left = json_body(left).await;
    let right = json_body(right).await;
    assert_eq!(left, right);
    assert_eq!(left["mode"], "cheat_sheet_only");
    assert!(left["content"].as_str().unwrap().contains("치트 시트"));
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM project_learning_events WHERE idempotency_key=$1")
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 1);
    let attempt = activity_attempt(&pool, user.user_id, false, None).await;
    let evidence_uri = format!("{uri}/evidence");
    assert_eq!(post(&app,&user,&evidence_uri,json!({"milestone_id":cheat_project["milestones"][0]["id"],"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":attempt,"idempotency_key":key})).await.status(),StatusCode::CONFLICT);
    let evidence_key = Uuid::now_v7();
    let failed = activity_attempt(&pool, user.user_id, false, None).await;
    let evidence = json!({"milestone_id":cheat_project["milestones"][0]["id"],"skill":"typescript","kind":"attempt","successful":false,"source_attempt_id":failed,"idempotency_key":evidence_key});
    let (left, right) = tokio::join!(
        post(&app, &user, &evidence_uri, evidence.clone()),
        post(&app, &user, &evidence_uri, evidence)
    );
    assert_eq!(left.status(), StatusCode::OK);
    assert_eq!(right.status(), StatusCode::OK);
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM project_learning_events WHERE idempotency_key=$1")
            .bind(evidence_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 1);
    let transfer = project(&app, &user, "transfer_challenge").await;
    let help=json_body(post(&app,&user,&format!("/api/v1/projects/{}/assistance",transfer["id"].as_str().unwrap()),json!({"milestone_id":transfer["milestones"][0]["id"],"skill":"typescript","idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(help["mode"], "transfer_challenge");
    assert!(help["content"].as_str().unwrap().contains("다른 맥락"));
}

#[sqlx::test(migrations = "./migrations")]
async fn plan_history_restore_and_import_metadata_fail_closed(pool: PgPool) {
    let app = app(pool);
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
    let _ = post(
        &app,
        &user,
        "/api/v1/learning/plans",
        plan_request(Uuid::now_v7()),
    )
    .await;
    let restore_key = Uuid::now_v7();
    let restore_uri = format!("/api/v1/learning/plans/{}/restore", first["revision"]);
    let (left, right) = tokio::join!(
        post(
            &app,
            &user,
            &restore_uri,
            json!({"idempotency_key":restore_key})
        ),
        post(
            &app,
            &user,
            &restore_uri,
            json!({"idempotency_key":restore_key})
        )
    );
    assert!(matches!(
        left.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    assert!(matches!(
        right.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    let restored = json_body(left).await;
    assert_eq!(restored["id"], json_body(right).await["id"]);
    assert_eq!(restored["restored_from_id"], first["id"]);
    assert_eq!(restored["plan_hash"], first["plan_hash"]);
    let history = json_body(get(&app, &user, "/api/v1/learning/plans").await).await;
    assert_eq!(history["revisions"].as_array().unwrap().len(), 3);
    let mut imported = idea_request(Uuid::now_v7(), vec!["기록"], "independent");
    imported["source_kind"] = json!("imported");
    assert_eq!(
        post(&app, &user, "/api/v1/projects/ideas", imported)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut valid = idea_request(Uuid::now_v7(), vec!["기록"], "independent");
    valid["source_kind"] = json!("imported");
    valid["repository_url"] = json!("https://github.com/example/owned");
    valid["repository_revision"] = json!("abcdef1234567");
    valid["ownership_basis"] = json!("learner_owned_repository");
    valid["license_identifier"] = json!("MIT");
    assert_eq!(
        post(&app, &user, "/api/v1/projects/ideas", valid)
            .await
            .status(),
        StatusCode::CREATED
    );
}
