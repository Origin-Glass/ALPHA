use std::collections::HashMap;

use alpha::{
    config::Settings,
    content_factory::{
        CompleteGenerationError, complete_generation_job, fail_generation_job,
        lease_next_generation_job,
    },
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

async fn app(pool: PgPool, credential: bool) -> (axum::Router, Session) {
    let mut settings = HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
        ("CONTENT_AI_ENABLED", "true"),
    ]);
    if credential {
        settings.insert("CONTENT_AI_CREDENTIALS_AVAILABLE", "OPENAI_API_KEY");
    }
    let app = router(AppState::new(
        pool.clone(),
        Settings::from_pairs(settings).unwrap(),
    ));
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"handle": "content-operator"}).to_string(),
                ))
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
    let session = Session {
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    };
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'ADMIN')")
        .bind(session.user_id)
        .execute(&pool)
        .await
        .unwrap();
    let consent = post(
        &app,
        &session,
        "/api/v1/policies/consents",
        json!({
            "version": "2026-07-18",
            "choices": {"terms": true, "privacy": true}
        }),
    )
    .await;
    assert_eq!(consent.status(), StatusCode::OK);
    (app, session)
}

async fn additional_session(app: &axum::Router, pool: &PgPool, handle: &str) -> Session {
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
    let session = Session {
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    };
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'CONTENT_CREATOR')")
        .bind(session.user_id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        post(
            app,
            &session,
            "/api/v1/policies/consents",
            json!({"version": "2026-07-18", "choices": {"terms": true, "privacy": true}}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    session
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

async fn get(app: &axum::Router, session: &Session, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
                .header(header::COOKIE, &session.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn put(
    app: &axum::Router,
    session: &Session,
    uri: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::put(uri)
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
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn provider(name: &str, kind: &str, base_url: &str) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "protocol": "openai_compatible",
        "base_url": base_url,
        "model": "test-model",
        "cost_per_generation_microunits": 80,
        "credential_env_var": if kind == "local" { Value::Null } else { json!("OPENAI_API_KEY") },
        "enabled": true
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn reservation_uses_server_provider_price_not_client_estimate(pool: PgPool) {
    let (app, session) = app(pool.clone(), true).await;
    let mut priced_provider = provider("server-priced", "external", "https://api.example.com");
    priced_provider["cost_per_generation_microunits"] = json!(30);
    let created = post(&app, &session, "/api/v1/content/providers", priced_provider).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let provider_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let created_job = post(
        &app,
        &session,
        "/api/v1/content/jobs",
        json!({
            "provider_id": provider_id,
            "content_type": "code_reading",
            "topic": "서버 가격 기반 예약",
            "target_language": "ko",
            "generation_count": 2
        }),
    )
    .await;
    assert_eq!(created_job.status(), StatusCode::CREATED);
    let reserved: i64 = sqlx::query_scalar(
        "SELECT reserved_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reserved, 180);

    let mut zero_price = provider("zero-price", "external", "https://api.example.com");
    zero_price["cost_per_generation_microunits"] = json!(0);
    assert_eq!(
        post(&app, &session, "/api/v1/content/providers", zero_price)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn content_creator_can_use_enabled_frontier_provider(pool: PgPool) {
    let (app, session) = app(pool.clone(), true).await;
    let created = post(
        &app,
        &session,
        "/api/v1/content/providers",
        provider("creator-frontier", "external", "https://api.example.com"),
    )
    .await;
    let provider_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    sqlx::query("DELETE FROM user_roles WHERE user_id = $1 AND role = 'ADMIN'")
        .bind(session.user_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'CONTENT_CREATOR')")
        .bind(session.user_id)
        .execute(&pool)
        .await
        .unwrap();

    let response = post(
        &app,
        &session,
        "/api/v1/content/jobs",
        json!({
            "provider_id": provider_id,
            "content_type": "debugging",
            "topic": "작성자 생성 권한",
            "target_language": "ko",
            "generation_count": 1
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[sqlx::test(migrations = "./migrations")]
async fn expensive_fallback_is_snapshotted_and_fully_reserved(pool: PgPool) {
    let (app, owner) = app(pool.clone(), true).await;
    let mut primary = provider(
        "fallback-primary",
        "external",
        "https://primary.example.com",
    );
    primary["cost_per_generation_microunits"] = json!(30);
    let primary_id: Uuid =
        json_body(post(&app, &owner, "/api/v1/content/providers", primary).await).await["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    let mut fallback = provider(
        "fallback-expensive",
        "external",
        "https://fallback.example.com",
    );
    fallback["cost_per_generation_microunits"] = json!(80);
    let fallback_id: Uuid =
        json_body(post(&app, &owner, "/api/v1/content/providers", fallback).await).await["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(
        put(
            &app, &owner, &format!("/api/v1/content/providers/{primary_id}"),
            json!({"enabled": true, "supports_stream": false, "supports_tools": false, "fallback_provider_id": fallback_id}),
        ).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        put(
            &app, &owner, &format!("/api/v1/content/providers/{fallback_id}"),
            json!({"enabled": true, "supports_stream": false, "supports_tools": false, "fallback_provider_id": primary_id}),
        ).await.status(),
        StatusCode::BAD_REQUEST
    );
    let mut third = provider("fallback-third", "external", "https://third.example.com");
    third["cost_per_generation_microunits"] = json!(40);
    let third_id: Uuid = json_body(post(&app, &owner, "/api/v1/content/providers", third).await)
        .await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        put(
            &app,
            &owner,
            &format!("/api/v1/content/providers/{fallback_id}"),
            json!({"enabled": true, "supports_stream": false, "supports_tools": false, "fallback_provider_id": third_id}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        put(
            &app,
            &owner,
            &format!("/api/v1/content/providers/{third_id}"),
            json!({"enabled": true, "supports_stream": false, "supports_tools": false, "fallback_provider_id": primary_id}),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put(
            &app, &owner, &format!("/api/v1/content/providers/{primary_id}"),
            json!({"enabled": true, "supports_stream": true, "supports_tools": false, "fallback_provider_id": fallback_id}),
        ).await.status(),
        StatusCode::BAD_REQUEST
    );
    let created = post(
        &app, &owner, "/api/v1/content/jobs",
        json!({"provider_id": primary_id, "content_type": "debugging", "topic": "fallback snapshot", "target_language": "ko", "generation_count": 2}),
    ).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let job_id: Uuid = json_body(created).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let snapshot: (Option<Uuid>, i64, i64) = sqlx::query_as(
        "SELECT fallback_provider_id_snapshot, attempt_cost_microunits, reserved_cost_microunits FROM content_generation_jobs WHERE id = $1",
    ).bind(job_id).fetch_one(&pool).await.unwrap();
    assert_eq!(snapshot, (Some(fallback_id), 160, 480));

    assert_eq!(
        put(
            &app, &owner, &format!("/api/v1/content/providers/{primary_id}"),
            json!({"enabled": true, "supports_stream": false, "supports_tools": false, "fallback_provider_id": null}),
        ).await.status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE content_provider_configs SET health_status = 'unhealthy' WHERE id = $1")
        .bind(primary_id)
        .execute(&pool)
        .await
        .unwrap();
    let lease =
        lease_next_generation_job(&pool, "fallback-worker", std::time::Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(lease.provider_id, fallback_id);
}

#[sqlx::test(migrations = "./migrations")]
async fn owner_job_lifecycle_is_scoped_idempotent_and_budget_safe(pool: PgPool) {
    let (app, owner) = app(pool.clone(), true).await;
    let intruder = additional_session(&app, &pool, "content-intruder").await;
    let job_id = queued_job(&app, &owner, "lifecycle-provider").await;
    assert_eq!(
        get(&app, &intruder, &format!("/api/v1/content/jobs/{job_id}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for action in ["cancel", "clone", "archive"] {
        assert_eq!(
            post(
                &app,
                &intruder,
                &format!("/api/v1/content/jobs/{job_id}/{action}"),
                json!({"idempotency_key": Uuid::now_v7()}),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "{action} leaked or changed another owner's job"
        );
    }
    let cancel_key = Uuid::now_v7();
    for _ in 0..2 {
        assert_eq!(
            post(
                &app,
                &owner,
                &format!("/api/v1/content/jobs/{job_id}/cancel"),
                json!({"idempotency_key": cancel_key})
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0));
    let cancel_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'content_job.cancelled' AND target_id = $1",
    )
    .bind(job_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cancel_audits, 1);

    let clone_key = Uuid::now_v7();
    let first_clone = json_body(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{job_id}/clone"),
            json!({"idempotency_key": clone_key}),
        )
        .await,
    )
    .await;
    let replay_clone = json_body(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{job_id}/clone"),
            json!({"idempotency_key": clone_key}),
        )
        .await,
    )
    .await;
    assert_eq!(first_clone["id"], replay_clone["id"]);
    assert_eq!(replay_clone["idempotent_replay"], true);

    let archive_key = Uuid::now_v7();
    for _ in 0..2 {
        assert_eq!(
            post(
                &app,
                &owner,
                &format!("/api/v1/content/jobs/{job_id}/archive"),
                json!({"idempotency_key": archive_key})
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let jobs = json_body(get(&app, &owner, "/api/v1/content/jobs").await).await;
    assert!(
        jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job["id"] != job_id.to_string())
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn leased_cancel_charges_attempt_and_fences_stale_worker(pool: PgPool) {
    let (app, owner) = app(pool.clone(), true).await;
    let job_id = queued_job(&app, &owner, "leased-cancel-provider").await;
    let lease = lease_next_generation_job(
        &pool,
        "content-worker-cancelled",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(lease.job_id, job_id);
    assert_eq!(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{job_id}/cancel"),
            json!({"idempotency_key": Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(matches!(
        complete_generation_job(
            &pool,
            lease.job_id,
            lease.attempt_id,
            lease.lease_token,
            json!({"candidate": "취소 후 stale output"}),
        )
        .await,
        Err(CompleteGenerationError::LeaseLost)
    ));
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 80));
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM content_generation_attempts WHERE id = $1")
            .bind(lease.attempt_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempt_status, "cancelled");
    let detail =
        json_body(get(&app, &owner, &format!("/api/v1/content/jobs/{job_id}")).await).await;
    assert_eq!(detail["attempts"][0]["status"], "cancelled");
    assert!(detail["attempts"][0]["request_hash"].as_str().is_some());
    assert!(
        detail["attempts"][0]["provider_snapshot"]["prompt_template_hash"]
            .as_str()
            .is_some()
    );
    assert!(
        detail["audit"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "content_job.cancelled")
    );
    let artifacts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM content_artifacts WHERE job_id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(artifacts, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn manual_retry_and_compare_are_owner_scoped_and_auditable(pool: PgPool) {
    let (app, owner) = app(pool.clone(), true).await;
    let intruder = additional_session(&app, &pool, "retry-intruder").await;
    let failed_id = queued_job(&app, &owner, "manual-retry-provider").await;
    let failed_lease =
        lease_next_generation_job(&pool, "retry-worker", std::time::Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
    fail_generation_job(
        &pool,
        failed_lease.job_id,
        failed_lease.attempt_id,
        failed_lease.lease_token,
        "terminal_provider_error",
        json!({"error": "safe terminal receipt"}),
        false,
    )
    .await
    .unwrap();
    let retry_key = Uuid::now_v7();
    assert_eq!(
        post(
            &app,
            &intruder,
            &format!("/api/v1/content/jobs/{failed_id}/retry"),
            json!({"idempotency_key": retry_key})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let retry = json_body(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{failed_id}/retry"),
            json!({"idempotency_key": retry_key}),
        )
        .await,
    )
    .await;
    let replay = json_body(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{failed_id}/retry"),
            json!({"idempotency_key": retry_key}),
        )
        .await,
    )
    .await;
    assert_eq!(retry["id"], replay["id"]);
    assert_eq!(replay["idempotent_replay"], true);
    let retried_id: Uuid = retry["id"].as_str().unwrap().parse().unwrap();
    let budget: (i64, i64) = sqlx::query_as("SELECT reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(budget, (240, 80));
    let retry_lease = lease_next_generation_job(
        &pool,
        "retry-success-worker",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(retry_lease.job_id, retried_id);
    complete_generation_job(
        &pool,
        retry_lease.job_id,
        retry_lease.attempt_id,
        retry_lease.lease_token,
        json!({"candidates": [{"content": {"title": "retry success"}}]}),
    )
    .await
    .unwrap();

    let clone_key = Uuid::now_v7();
    let clone = json_body(
        post(
            &app,
            &owner,
            &format!("/api/v1/content/jobs/{retried_id}/clone"),
            json!({"idempotency_key": clone_key}),
        )
        .await,
    )
    .await;
    let clone_id: Uuid = clone["id"].as_str().unwrap().parse().unwrap();
    let clone_lease = lease_next_generation_job(
        &pool,
        "clone-success-worker",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(clone_lease.job_id, clone_id);
    complete_generation_job(
        &pool,
        clone_lease.job_id,
        clone_lease.attempt_id,
        clone_lease.lease_token,
        json!({"candidates": [{"content": {"title": "clone success"}}]}),
    )
    .await
    .unwrap();
    let comparison = get(
        &app,
        &owner,
        &format!("/api/v1/content/jobs/compare?left={retried_id}&right={clone_id}"),
    )
    .await;
    assert_eq!(comparison.status(), StatusCode::OK);
    let comparison = json_body(comparison).await;
    assert_eq!(comparison["jobs"].as_array().unwrap().len(), 2);
    assert!(comparison.to_string().contains("retry success"));
    assert!(comparison.to_string().contains("clone success"));
    assert_eq!(
        get(
            &app,
            &intruder,
            &format!("/api/v1/content/jobs/compare?left={retried_id}&right={clone_id}"),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_configuration_rejects_ssrf_endpoints(pool: PgPool) {
    let (app, session) = app(pool.clone(), false).await;
    for (index, (kind, url)) in [
        ("external", "http://api.example.com"),
        ("external", "https://user:password@api.example.com"),
        ("external", "https://api.example.com:8443"),
        ("external", "https://127.0.0.1"),
        ("external", "https://10.0.0.8"),
        ("external", "https://169.254.169.254"),
        ("local", "https://api.example.com:11434"),
        ("local", "https://127.0.0.1:11434"),
        ("local", "http://127.0.0.2:11434"),
        ("local", "https://127.0.0.1:443"),
    ]
    .into_iter()
    .enumerate()
    {
        let response = post(
            &app,
            &session,
            "/api/v1/content/providers",
            provider(&format!("invalid-{index}"), kind, url),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "accepted {url}");
    }

    assert_eq!(
        post(
            &app,
            &session,
            "/api/v1/content/providers",
            provider("frontier", "external", "https://api.example.com")
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let mut anthropic = provider("anthropic", "external", "https://api.anthropic.com");
    anthropic["protocol"] = json!("anthropic");
    anthropic["credential_env_var"] = json!("ANTHROPIC_API_KEY");
    assert_eq!(
        post(&app, &session, "/api/v1/content/providers", anthropic)
            .await
            .status(),
        StatusCode::CREATED
    );
    let anthropic_protocol: String = sqlx::query_scalar(
        "SELECT protocol FROM content_provider_configs WHERE name = 'anthropic'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(anthropic_protocol, "anthropic");
    let mut secret_reference = provider(
        "forbidden-secret-reference",
        "external",
        "https://api.example.com",
    );
    secret_reference["credential_env_var"] = json!("SESSION_SECRET");
    assert_eq!(
        post(
            &app,
            &session,
            "/api/v1/content/providers",
            secret_reference
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(
            &app,
            &session,
            "/api/v1/content/providers",
            provider("local", "local", "http://127.0.0.1:11434")
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        post(
            &app,
            &session,
            "/api/v1/content/providers",
            provider("local-v6", "local", "http://[::1]:11434")
        )
        .await
        .status(),
        StatusCode::CREATED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_credential_blocks_job_without_artifact_or_secret_persistence(pool: PgPool) {
    let (app, session) = app(pool.clone(), false).await;
    let created = post(
        &app,
        &session,
        "/api/v1/content/providers",
        provider("missing-key", "external", "https://api.example.com"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let provider_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let provider_list = json_body(get(&app, &session, "/api/v1/content/providers").await).await;
    let visible_provider = &provider_list["providers"][0];
    assert_eq!(visible_provider["credential_available"], false);
    assert!(visible_provider.get("base_url").is_none());
    assert!(visible_provider.get("credential_env_var").is_none());

    let job = post(
        &app,
        &session,
        "/api/v1/content/jobs",
        json!({
            "provider_id": provider_id,
            "content_type": "code_reading",
            "topic": "이진 탐색 경계 오류",
            "target_language": "ko",
            "generation_count": 1
        }),
    )
    .await;
    assert_eq!(job.status(), StatusCode::CREATED);
    let job = json_body(job).await;
    assert_eq!(job["status"], "blocked_missing_credential");

    let artifact_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM content_artifacts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(artifact_count, 0);
    let persisted: String = sqlx::query_scalar(
        "SELECT row_to_json(provider)::text FROM content_provider_configs provider WHERE id = $1",
    )
    .bind(Uuid::parse_str(&provider_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(metadata::text, ''), '') FROM audit_events WHERE actor_user_id = $1",
    )
    .bind(session.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!persisted.contains("integration-only-secret"));
    assert!(!audit.contains("integration-only-secret"));
    assert!(!audit.to_ascii_lowercase().contains("authorization"));
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_jobs_cannot_reserve_beyond_one_budget(pool: PgPool) {
    let (app, session) = app(pool.clone(), true).await;
    sqlx::query("UPDATE content_budgets SET limit_microunits = 300 WHERE budget_key = 'global'")
        .execute(&pool)
        .await
        .unwrap();
    let created = post(
        &app,
        &session,
        "/api/v1/content/providers",
        provider("budgeted", "external", "https://api.example.com"),
    )
    .await;
    let provider_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let payload = json!({
        "provider_id": provider_id,
        "content_type": "debugging",
        "topic": "상태 전이 오류",
        "target_language": "ko",
        "generation_count": 1
    });

    let (first, second) = tokio::join!(
        post(&app, &session, "/api/v1/content/jobs", payload.clone()),
        post(&app, &session, "/api/v1/content/jobs", payload),
    );
    let mut statuses = [first.status(), second.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(budget, (240, 0));
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM content_generation_jobs WHERE status = 'queued'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 1);
}

async fn queued_job(app: &axum::Router, session: &Session, name: &str) -> Uuid {
    let created = post(
        app,
        session,
        "/api/v1/content/providers",
        provider(name, "external", "https://api.example.com"),
    )
    .await;
    let provider_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let job = post(
        app,
        session,
        "/api/v1/content/jobs",
        json!({
            "provider_id": provider_id,
            "content_type": "code_reading",
            "topic": "재시도 산출물 보존",
            "target_language": "ko",
            "generation_count": 1
        }),
    )
    .await;
    assert_eq!(job.status(), StatusCode::CREATED);
    json_body(job).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn retry_keeps_attempt_artifacts_and_settles_budget_once(pool: PgPool) {
    let (app, session) = app(pool.clone(), true).await;
    let job_id = queued_job(&app, &session, "retry-artifacts").await;
    let first = lease_next_generation_job(
        &pool,
        "content-worker-a",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    fail_generation_job(
        &pool,
        first.job_id,
        first.attempt_id,
        first.lease_token,
        "provider_timeout",
        json!({"partial": "첫 번째 응답"}),
        true,
    )
    .await
    .unwrap();

    let retry_delay_seconds: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (available_at - now()))::float8 FROM content_generation_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((0.0..=2.0).contains(&retry_delay_seconds));
    sqlx::query("UPDATE content_generation_jobs SET available_at = now() WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .unwrap();

    let second = lease_next_generation_job(
        &pool,
        "content-worker-b",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(second.job_id, job_id);
    assert_eq!(second.attempt_number, 2);
    complete_generation_job(
        &pool,
        second.job_id,
        second.attempt_id,
        second.lease_token,
        json!({"candidate": "두 번째 완성 응답"}),
    )
    .await
    .unwrap();
    assert!(matches!(
        complete_generation_job(
            &pool,
            second.job_id,
            second.attempt_id,
            second.lease_token,
            json!({"candidate": "중복 완료"}),
        )
        .await,
        Err(CompleteGenerationError::LeaseLost)
    ));

    let attempts: Vec<(i16, String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT attempt_number, status, response_hash FROM content_generation_attempts WHERE job_id = $1 ORDER BY attempt_number",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].1, "retryable_failure");
    assert_eq!(attempts[1].1, "completed");
    assert_ne!(attempts[0].2, attempts[1].2);
    let snapshots: Vec<Value> = sqlx::query_scalar(
        "SELECT provider_snapshot FROM content_generation_attempts WHERE job_id = $1 ORDER BY attempt_number",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot["protocol"] == "openai_compatible")
    );
    let artifacts: Vec<(i16, Vec<u8>, Value)> = sqlx::query_as(
        r#"SELECT attempt.attempt_number, artifact.content_hash, artifact.payload
           FROM content_artifacts artifact
           JOIN content_generation_attempts attempt ON attempt.id = artifact.attempt_id
           WHERE artifact.job_id = $1 ORDER BY attempt.attempt_number"#,
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_ne!(artifacts[0].1, artifacts[1].1);
    assert_eq!(artifacts[0].2["partial"], "첫 번째 응답");
    assert_eq!(artifacts[1].2["candidate"], "두 번째 완성 응답");
    let artifact_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM content_artifacts WHERE job_id = $1 ORDER BY created_at LIMIT 1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query("UPDATE content_artifacts SET payload = '{}'::jsonb WHERE id = $1")
            .bind(artifact_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM content_artifacts WHERE id = $1")
            .bind(artifact_id)
            .execute(&pool)
            .await
            .is_err()
    );
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT reserved_microunits, spent_microunits FROM content_budgets WHERE budget_key = 'global'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 160));
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_lease_recovery_fences_stale_worker_artifacts(pool: PgPool) {
    let (app, session) = app(pool.clone(), true).await;
    let job_id = queued_job(&app, &session, "lease-fencing").await;
    let stale = lease_next_generation_job(
        &pool,
        "content-worker-stale",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    sqlx::query(
        "UPDATE content_generation_jobs SET lease_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await
    .unwrap();
    let delayed = lease_next_generation_job(
        &pool,
        "content-worker-recovery",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(delayed.is_none());
    let recovery_delay_seconds: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (available_at - now()))::float8 FROM content_generation_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((0.0..=2.0).contains(&recovery_delay_seconds));
    sqlx::query("UPDATE content_generation_jobs SET available_at = now() WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .unwrap();
    let recovered = lease_next_generation_job(
        &pool,
        "content-worker-recovery",
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(recovered.job_id, job_id);
    assert_eq!(recovered.attempt_number, 2);
    assert!(matches!(
        complete_generation_job(
            &pool,
            stale.job_id,
            stale.attempt_id,
            stale.lease_token,
            json!({"candidate": "stale worker output"}),
        )
        .await,
        Err(CompleteGenerationError::LeaseLost)
    ));
    complete_generation_job(
        &pool,
        recovered.job_id,
        recovered.attempt_id,
        recovered.lease_token,
        json!({"candidate": "recovered worker output"}),
    )
    .await
    .unwrap();
    let payloads: Vec<Value> =
        sqlx::query_scalar("SELECT payload FROM content_artifacts WHERE job_id = $1")
            .bind(job_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        payloads,
        vec![json!({"candidate": "recovered worker output"})]
    );
}
