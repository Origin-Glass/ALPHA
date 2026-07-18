use std::collections::HashMap;

use alpha::{
    auth::{
        IdentityResolutionError, OAuthProfile, consume_oauth_transaction, resolve_oauth_identity,
    },
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
use url::Url;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn logout_requires_csrf_and_immediately_invalidates_the_session(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool, settings));

    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"handle": "verified-learner"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::CREATED);
    let session_cookie = login
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
        serde_json::from_slice(&to_bytes(login.into_body(), 16_384).await.unwrap()).unwrap();
    let csrf = login_body["csrf_token"].as_str().unwrap();

    let me = app
        .clone()
        .oneshot(
            Request::get("/api/v1/auth/me")
                .header(header::COOKIE, &session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);

    let rejected_logout = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/logout")
                .header(header::COOKIE, &session_cookie)
                .header("x-csrf-token", "wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected_logout.status(), StatusCode::FORBIDDEN);

    let accepted_terms = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/terms")
                .header(header::COOKIE, &session_cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"version": "2026-07-18"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted_terms.status(), StatusCode::OK);
    let accepted_body: Value =
        serde_json::from_slice(&to_bytes(accepted_terms.into_body(), 16_384).await.unwrap())
            .unwrap();
    assert_eq!(accepted_body["terms_accepted"], true);

    let valid_logout = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/logout")
                .header(header::COOKIE, &session_cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(valid_logout.status(), StatusCode::NO_CONTENT);

    let after_logout = app
        .oneshot(
            Request::get("/api/v1/auth/me")
                .header(header::COOKIE, session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_state_is_hashed_and_can_only_be_consumed_once(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        ("PUBLIC_BASE_URL", "https://alpha.example"),
        ("GOOGLE_CLIENT_ID", "google-client"),
        ("GOOGLE_CLIENT_SECRET", "google-secret"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));

    let response = app
        .oneshot(
            Request::get("/api/v1/auth/google/start?redirect_after=/learn")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    let authorization_url = Url::parse(location).unwrap();
    assert_eq!(authorization_url.host_str(), Some("accounts.google.com"));
    let state = authorization_url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    assert!(
        authorization_url
            .query_pairs()
            .any(|(key, _)| key == "code_challenge")
    );

    let raw_state_was_stored: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oauth_transactions WHERE state_hash = $1)")
            .bind(state.as_bytes())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!raw_state_was_stored);

    let secret = "0123456789abcdef0123456789abcdef";
    let first = consume_oauth_transaction(&pool, "google", &state, secret)
        .await
        .unwrap();
    let replay = consume_oauth_transaction(&pool, "google", &state, secret)
        .await
        .unwrap();
    assert_eq!(first.unwrap().redirect_after, "/learn");
    assert!(replay.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn matching_email_never_auto_links_different_oauth_subjects(pool: PgPool) {
    let google_user = resolve_oauth_identity(
        &pool,
        OAuthProfile {
            provider: "google".to_owned(),
            subject: "google-subject".to_owned(),
            email: Some("learner@example.com".to_owned()),
            email_verified: true,
            display_name: "학습자".to_owned(),
            handle_hint: "learner".to_owned(),
        },
    )
    .await
    .unwrap();

    let collision = resolve_oauth_identity(
        &pool,
        OAuthProfile {
            provider: "github".to_owned(),
            subject: "github-subject".to_owned(),
            email: Some("LEARNER@example.com".to_owned()),
            email_verified: true,
            display_name: "다른 계정".to_owned(),
            handle_hint: "other".to_owned(),
        },
    )
    .await;

    assert!(matches!(
        collision,
        Err(IdentityResolutionError::EmailCollision)
    ));
    let identity_count: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_identities")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(identity_count, 1);
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM oauth_identities WHERE provider = 'google'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        google_user
    );
}
