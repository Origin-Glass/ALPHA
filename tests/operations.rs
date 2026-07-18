use std::{process::Command, str::FromStr};

use alpha::{
    config::Settings,
    http::{AppState, router},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

struct Session {
    user_id: Uuid,
    cookie: String,
}

async fn admin_session(app: &axum::Router, pool: &PgPool) -> Session {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle":"operations-admin"}).to_string()))
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
    let user_id = body["user"]["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'ADMIN')")
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
    Session { user_id, cookie }
}

async fn provider(pool: &PgPool, creator: Uuid, name: &str, health: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO content_provider_configs (name,kind,protocol,base_url,model,cost_per_generation_microunits,credential_env_var,enabled,health_status,last_health_at,created_by) VALUES ($1,'external','openai_compatible','https://secret-provider.example','secret-model',1,'OPENAI_API_KEY',true,$2,now(),$3) RETURNING id",
    )
    .bind(name)
    .bind(health)
    .bind(creator)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn generation_job(pool: &PgPool, creator: Uuid, provider_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO content_generation_jobs (created_by,provider_id,content_type,request_spec,request_hash,status,estimated_cost_microunits,attempt_cost_microunits,reserved_cost_microunits,attempt_count,max_attempts,lease_owner,lease_token,lease_expires_at) VALUES ($1,$2,'algorithm_problem',$3,$4,'leased',3,1,3,1,3,'dead-worker',$5,now()-interval '1 minute') RETURNING id",
    )
    .bind(creator)
    .bind(provider_id)
    .bind(json!({"private_prompt":"must-never-leak"}))
    .bind(vec![7_u8; 32])
    .bind(Uuid::now_v7())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_operations_identifies_expired_provider_job_without_exposing_configuration(
    pool: PgPool,
) {
    let settings = Settings::from_pairs(std::collections::HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let admin = admin_session(&app, &pool).await;
    let provider_id = provider(&pool, admin.user_id, "operations-provider", "unhealthy").await;
    generation_job(&pool, admin.user_id, provider_id).await;

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/admin/operations")
                .header(header::COOKIE, admin.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["providers"]["unhealthy"], 1);
    assert_eq!(payload["generation_jobs"]["running"], 1);
    assert_eq!(payload["generation_jobs"]["expired_leases"], 1);
    assert!(!body.contains("secret-provider.example"));
    assert!(!body.contains("secret-model"));
    assert!(!body.contains("must-never-leak"));
    assert!(!body.contains("OPENAI_API_KEY"));

    let metrics = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let metrics = String::from_utf8(
        to_bytes(metrics.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(metrics.contains("alpha_content_providers{state=\"unhealthy\"} 1"));
    assert!(metrics.contains("alpha_content_generation_jobs{state=\"expired_lease\"} 1"));
}

async fn review_candidate(pool: &PgPool, suffix: &str, with_rights: bool) -> Uuid {
    let author: Uuid =
        sqlx::query_scalar("INSERT INTO users (handle,display_name) VALUES ($1,$2) RETURNING id")
            .bind(format!("author-{suffix}"))
            .bind(format!("Author {suffix}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let reviewer: Uuid =
        sqlx::query_scalar("INSERT INTO users (handle,display_name) VALUES ($1,$2) RETURNING id")
            .bind(format!("reviewer-{suffix}"))
            .bind(format!("Reviewer {suffix}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let provider_id: Uuid = sqlx::query_scalar("INSERT INTO content_provider_configs (name,kind,protocol,base_url,model,cost_per_generation_microunits,credential_env_var,created_by) VALUES ($1,'local','openai_compatible','http://127.0.0.1:11434','gate-model',1,NULL,$2) RETURNING id")
        .bind(format!("gate-{suffix}"))
        .bind(author)
        .fetch_one(pool)
        .await
        .unwrap();
    let request_hash = Sha256::digest(format!("request-{suffix}").as_bytes()).to_vec();
    let job_id: Uuid = sqlx::query_scalar("INSERT INTO content_generation_jobs (created_by,provider_id,content_type,request_spec,request_hash,status,estimated_cost_microunits,attempt_cost_microunits,max_attempts,settled) VALUES ($1,$2,'algorithm_problem',$3,$4,'completed',3,1,3,true) RETURNING id")
        .bind(author)
        .bind(provider_id)
        .bind(json!({"topic":suffix}))
        .bind(&request_hash)
        .fetch_one(pool)
        .await
        .unwrap();
    let attempt_id: Uuid = sqlx::query_scalar("INSERT INTO content_generation_attempts (job_id,attempt_number,provider_snapshot,request_hash,response_hash,status,completed_at) VALUES ($1,1,$2,$3,$4,'completed',now()) RETURNING id")
        .bind(job_id)
        .bind(json!({"provider_id":provider_id,"model":"gate-model"}))
        .bind(&request_hash)
        .bind(vec![3_u8; 32])
        .fetch_one(pool)
        .await
        .unwrap();
    let artifact_hash = Sha256::digest(format!("artifact-{suffix}").as_bytes()).to_vec();
    let artifact_id: Uuid = sqlx::query_scalar("INSERT INTO content_artifacts (job_id,attempt_id,artifact_kind,payload,content_hash) VALUES ($1,$2,'candidate',$3,$4) RETURNING id")
        .bind(job_id)
        .bind(attempt_id)
        .bind(json!({"title":suffix}))
        .bind(&artifact_hash)
        .fetch_one(pool)
        .await
        .unwrap();
    let review_id: Uuid = sqlx::query_scalar("INSERT INTO content_review_items (artifact_id,artifact_hash,author_user_id,state,impact,rights_reviewer_id,create_idempotency_key) VALUES ($1,$2,$3,'approved','standard',$4,$5) RETURNING id")
        .bind(artifact_id)
        .bind(&artifact_hash)
        .bind(author)
        .bind(reviewer)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key,request_hash) VALUES ($1,1,$2,$3,$4,$5,$6)")
        .bind(review_id)
        .bind(artifact_id)
        .bind(&artifact_hash)
        .bind(author)
        .bind(Uuid::now_v7())
        .bind(&request_hash)
        .execute(pool)
        .await
        .unwrap();
    if with_rights {
        approve_rights(pool, review_id, reviewer, artifact_hash).await;
    }
    review_id
}

async fn approve_rights(pool: &PgPool, review_id: Uuid, reviewer: Uuid, artifact_hash: Vec<u8>) {
    let provenance_hash = Sha256::digest(review_id.as_bytes()).to_vec();
    let provenance_id: Uuid = sqlx::query_scalar("INSERT INTO content_provenance_records (review_item_id,revision_number,artifact_hash,origin_type,creator_or_provider,source_revision,license_basis,license_identifier,attribution,modification_status,commercial_use_allowed,redistribution_allowed,evidence_reference,evidence_hash,attachment_metadata,legal_status,provenance_hash,recorded_by,idempotency_key) VALUES ($1,1,$2,'original','ALPHA author','revision-1','copyright_owner','copyright-owner','ALPHA author','unmodified',true,true,'internal evidence',$3,'{}','pending',$4,$5,$6) RETURNING id")
        .bind(review_id)
        .bind(&artifact_hash)
        .bind(vec![5_u8; 32])
        .bind(&provenance_hash)
        .bind(reviewer)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_rights_review_receipts (review_item_id,revision_number,reviewer_user_id,provenance_id,provenance_hash,decision,basis,evidence,commercial_use_allowed,redistribution_allowed,provider_terms_version,idempotency_key,receipt_hash) VALUES ($1,1,$2,$3,$4,'approve','original','rights verified',true,true,'terms-v1',$5,$6)")
        .bind(review_id)
        .bind(reviewer)
        .bind(provenance_id)
        .bind(provenance_hash)
        .bind(Uuid::now_v7())
        .bind(vec![9_u8; 32])
        .execute(pool)
        .await
        .unwrap();
}

fn test_database_url(pool: &PgPool) -> String {
    let options = pool.connect_options();
    let database = options.get_database().unwrap();
    let mut url = Url::from_str(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(database);
    url.to_string()
}

#[sqlx::test(migrations = "./migrations")]
async fn publication_gate_cli_fails_for_one_unapproved_candidate_and_passes_after_approval(
    pool: PgPool,
) {
    review_candidate(&pool, "approved", true).await;
    let blocked = review_candidate(&pool, "blocked", false).await;
    let database_url = test_database_url(&pool);

    let binary = std::env::var("CARGO_BIN_EXE_publication_gate").unwrap();
    let failure = Command::new(&binary)
        .env("DATABASE_URL", &database_url)
        .status()
        .unwrap();
    assert!(!failure.success());

    let (reviewer, artifact_hash): (Uuid, Vec<u8>) = sqlx::query_as(
        "SELECT rights_reviewer_id,artifact_hash FROM content_review_items WHERE id=$1",
    )
    .bind(blocked)
    .fetch_one(&pool)
    .await
    .unwrap();
    approve_rights(&pool, blocked, reviewer, artifact_hash).await;
    let success = Command::new(binary)
        .env("DATABASE_URL", database_url)
        .status()
        .unwrap();
    assert!(success.success());
}
