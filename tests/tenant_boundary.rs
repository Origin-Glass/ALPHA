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

async fn test_login(app: &axum::Router, handle: &str) -> (String, Uuid) {
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
    (
        cookie,
        body["user"]["id"].as_str().unwrap().parse().unwrap(),
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn instructor_cannot_read_another_organizations_class(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let (instructor_a_cookie, instructor_a) = test_login(&app, "instructor-a").await;
    let (instructor_b_cookie, instructor_b) = test_login(&app, "instructor-b").await;
    let organization_a = Uuid::now_v7();
    let organization_b = Uuid::now_v7();
    let class_b = Uuid::now_v7();

    for (organization_id, slug, owner) in [
        (organization_a, "organization-a", instructor_a),
        (organization_b, "organization-b", instructor_b),
    ] {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, created_by) VALUES ($1, $2, $2, $3)",
        )
        .bind(organization_id)
        .bind(slug)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, user_id, role) VALUES ($1, $2, 'INSTRUCTOR')",
        )
        .bind(organization_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO classes (id, organization_id, name, created_by) VALUES ($1, $2, '비공개 학급', $3)",
    )
    .bind(class_b)
    .bind(organization_b)
    .bind(instructor_b)
    .execute(&pool)
    .await
    .unwrap();

    let cross_tenant = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_b}"))
                .header(header::COOKIE, &instructor_a_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);

    let cross_tenant_dashboard = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_b}/instructor-dashboard"))
                .header(header::COOKIE, &instructor_a_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant_dashboard.status(), StatusCode::NOT_FOUND);

    let own_tenant = app
        .oneshot(
            Request::get(format!("/api/v1/classes/{class_b}"))
                .header(header::COOKIE, instructor_b_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(own_tenant.status(), StatusCode::OK);
}
