use alpha::http::{AppState, router};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn public_problem_detail_never_leaks_hidden_tests(pool: PgPool) {
    let problem_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO problems (
            id, slug, title_ko, statement_ko, difficulty, learning_axis,
            status, source_kind, time_limit_ms, memory_limit_mb, published_at
        ) VALUES ($1, $2, $3, $4, 4, 'code_literacy', 'published', 'original', 1000, 256, now())
        "#,
    )
    .bind(problem_id)
    .bind("hidden-case-contract")
    .bind("숨은 테스트 계약")
    .bind("공개 문제 설명")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO problem_test_cases (id, problem_id, ordinal, input, expected_output, visibility)
        VALUES ($1, $2, 1, 'do-not-leak-input', 'do-not-leak-answer', 'hidden')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(problem_id)
    .execute(&pool)
    .await
    .unwrap();

    let response = router(AppState::for_test(pool))
        .oneshot(
            Request::builder()
                .uri("/api/v1/problems/hidden-case-contract")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let serialized = serde_json::to_string(&json).unwrap();

    assert_eq!(json["problem"]["title"], "숨은 테스트 계약");
    assert!(!serialized.contains("do-not-leak-input"));
    assert!(!serialized.contains("do-not-leak-answer"));
    assert!(json["problem"].get("test_cases").is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn catalog_pagination_has_no_duplicates_and_excludes_drafts(pool: PgPool) {
    for (id, slug, status, published_at) in [
        (
            "00000000-0000-0000-0000-000000000001",
            "catalog-one",
            "published",
            Some(OffsetDateTime::UNIX_EPOCH),
        ),
        (
            "00000000-0000-0000-0000-000000000002",
            "catalog-two",
            "published",
            Some(OffsetDateTime::UNIX_EPOCH),
        ),
        (
            "00000000-0000-0000-0000-000000000003",
            "catalog-three",
            "published",
            Some(OffsetDateTime::UNIX_EPOCH),
        ),
        (
            "00000000-0000-0000-0000-000000000004",
            "draft-must-stay-private",
            "draft",
            None,
        ),
    ] {
        sqlx::query(
            r#"
            INSERT INTO problems (
                id, slug, title_ko, statement_ko, difficulty, learning_axis,
                status, source_kind, time_limit_ms, memory_limit_mb, published_at
            ) VALUES ($1, $2, $2, '문제 설명', 1, 'algorithmic_reasoning', $3, 'original', 1000, 256, $4)
            "#,
        )
        .bind(Uuid::parse_str(id).unwrap())
        .bind(slug)
        .bind(status)
        .bind(published_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    let app = router(AppState::for_test(pool));
    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/problems?limit=2&q=catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    let first: Value = serde_json::from_slice(
        &first_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();

    let cursor = first["next_cursor"].as_str().unwrap();
    let second_response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/problems?limit=2&q=catalog&cursor={cursor}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let second: Value = serde_json::from_slice(
        &second_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();

    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    let slugs: Vec<_> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|item| item["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["catalog-three", "catalog-two", "catalog-one"]);
    let serialized = format!("{first}{second}");
    assert!(!serialized.contains("draft-must-stay-private"));
}
