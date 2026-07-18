use alpha::{
    config::Settings,
    http::{AppState, router},
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use tower::ServiceExt;

#[sqlx::test(migrations = "./migrations")]
async fn disabled_payments_create_no_checkout_or_entitlement(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("PAYMENTS_ENABLED", "false"),
        ("PAYMENT_PROVIDER", "disabled"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));

    let response = app
        .oneshot(
            Request::post("/api/v1/payments/checkout")
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "checkout-attempt-1")
                .body(Body::from(
                    json!({"plan": "premium_individual"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let checkout_count: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_intents")
        .fetch_one(&pool)
        .await
        .unwrap();
    let entitlement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM entitlement_grants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((checkout_count, entitlement_count), (0, 0));
}
