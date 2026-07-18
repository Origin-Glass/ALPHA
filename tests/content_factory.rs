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

async fn app(pool: PgPool, credential: bool) -> (axum::Router, Session) {
    let mut settings = HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
        ("CONTENT_AI_ENABLED", "true"),
    ]);
    if credential {
        settings.insert("OPENAI_API_KEY", "integration-only-secret");
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
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn provider(name: &str, kind: &str, base_url: &str) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "base_url": base_url,
        "model": "test-model",
        "credential_env_var": if kind == "local" { Value::Null } else { json!("OPENAI_API_KEY") },
        "enabled": true
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_configuration_rejects_ssrf_endpoints(pool: PgPool) {
    let (app, session) = app(pool, false).await;
    for (index, (kind, url)) in [
        ("external", "http://api.example.com"),
        ("external", "https://user:password@api.example.com"),
        ("external", "https://api.example.com:8443"),
        ("external", "https://127.0.0.1"),
        ("external", "https://10.0.0.8"),
        ("external", "https://169.254.169.254"),
        ("local", "https://api.example.com:11434"),
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
            provider("local", "local", "https://127.0.0.1:11434")
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

    let job = post(
        &app,
        &session,
        "/api/v1/content/jobs",
        json!({
            "provider_id": provider_id,
            "content_type": "code_reading",
            "topic": "이진 탐색 경계 오류",
            "target_language": "ko",
            "generation_count": 1,
            "estimated_cost_microunits": 50
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
    sqlx::query("UPDATE content_budgets SET limit_microunits = 100 WHERE budget_key = 'global'")
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
        "generation_count": 1,
        "estimated_cost_microunits": 80
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
    assert_eq!(budget, (80, 0));
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM content_generation_jobs WHERE status = 'queued'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 1);
}
