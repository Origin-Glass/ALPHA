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
    csrf: String,
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
    let csrf = body["csrf_token"].as_str().unwrap().to_owned();
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, 'ADMIN')")
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
    Session {
        user_id,
        cookie,
        csrf,
    }
}

async fn role_session(app: &axum::Router, pool: &PgPool, handle: &str, role: &str) -> Session {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/test-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle":handle}).to_string()))
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
    let session = Session {
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    };
    sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, $2)")
        .bind(session.user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        post(
            app,
            &session,
            "/api/v1/policies/consents",
            json!({"version":"2026-07-18","choices":{"terms":true,"privacy":true}}),
        )
        .await,
        StatusCode::OK
    );
    session
}

async fn post(app: &axum::Router, session: &Session, uri: &str, body: Value) -> StatusCode {
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
        .status()
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

async fn stalled_project(pool: &PgPool, user_id: Uuid) -> Uuid {
    let idea_id = Uuid::now_v7();
    sqlx::query("INSERT INTO project_ideas (id,user_id,revision,idempotency_key,title,motivation,target_user,intended_outcome,core_feature,technology,weekly_minutes,assistance_policy,skill_level,runtime,infrastructure,requested_features,scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,rule_version,lineage_id,input,input_hash,created_at) VALUES ($1,$2,1,$3,'ops idea','ops','operator','diagnose','diagnostics','rust',60,'independent','advanced','cli','local_only','[\"diagnose\"]','[\"diagnose\"]','[\"inspect\"]','[\"bounded\"]','[]',false,'project-learning-v1',$4,'{}',decode(repeat('11',32),'hex'),now()-interval '8 days')")
        .bind(idea_id)
        .bind(user_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap();
    let project_id = Uuid::now_v7();
    sqlx::query("INSERT INTO learner_projects (id,user_id,idea_id,idempotency_key,created_at) VALUES ($1,$2,$3,$4,now()-interval '8 days')")
        .bind(project_id)
        .bind(user_id)
        .bind(idea_id)
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_learning_events (id,user_id,project_id,event_kind,created_at) VALUES ($1,$2,$3,'project_created',now()-interval '7 days 1 hour')")
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(project_id)
        .execute(pool)
        .await
        .unwrap();
    project_id
}

async fn expired_workspace_run(pool: &PgPool, user_id: Uuid) -> Uuid {
    let workspace_id = Uuid::now_v7();
    sqlx::query("INSERT INTO project_workspaces (id,user_id,template_id,template_revision,template_digest,title,request_hash,idempotency_key,created_at,updated_at) SELECT $1,$2,id,revision,template_digest,'ops workspace',decode(repeat('21',32),'hex'),$3,now()-interval '1 hour',now()-interval '1 hour' FROM workspace_templates WHERE slug='python-cli-v1'")
        .bind(workspace_id)
        .bind(user_id)
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap();
    let run_id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspace_runs (id,workspace_id,user_id,workspace_version,template_id,template_revision,template_digest,image_reference,execution_image_digest,artifact,artifact_hash,semantic_hash,check_suite_id,check_suite_hash,supports_tests_snapshot,background_services_snapshot,dependency_cache_status,cache_digest,command,status,attempt,lease_token,leased_by,lease_expires_at,idempotency_key,created_at,request_hash) SELECT $1,$2,$3,1,id,revision,template_digest,image_reference,'sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','[]',decode(repeat('22',32),'hex'),decode(repeat('23',32),'hex'),check_suite_id,check_suite_hash,supports_tests,background_services,dependency_cache_status,cache_digest,run_command,'running',1,$4,'dead-workspace-worker',now()-interval '1 minute',$5,now()-interval '1 hour',decode(repeat('24',32),'hex') FROM workspace_templates WHERE slug='python-cli-v1'")
        .bind(run_id)
        .bind(workspace_id)
        .bind(user_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap();
    run_id
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
    let job_id = generation_job(&pool, admin.user_id, provider_id).await;
    for _ in 0..24 {
        generation_job(&pool, admin.user_id, provider_id).await;
    }
    let (review_id, _, _) =
        review_candidate(&pool, "operations-review", "rights_review_pending").await;
    let project_id = stalled_project(&pool, admin.user_id).await;
    let workspace_run_id = expired_workspace_run(&pool, admin.user_id).await;

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
    assert_eq!(payload["generation_jobs"]["running"], 25);
    assert_eq!(payload["generation_jobs"]["expired_leases"], 25);
    assert_eq!(
        payload["generation_jobs"]["action_items"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert_eq!(
        payload["providers"]["action_items"][0]["resource_id"],
        provider_id.to_string()
    );
    assert_eq!(
        payload["providers"]["action_items"][0]["reason"],
        "provider_unhealthy"
    );
    assert!(
        payload["providers"]["action_items"][0]["age_seconds"]
            .as_i64()
            .unwrap()
            >= 0
    );
    assert!(
        payload["generation_jobs"]["action_items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["reason"] == "expired_lease")
    );
    assert!(
        payload["generation_jobs"]["action_items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["resource_id"] == job_id.to_string())
    );
    assert_eq!(
        payload["reviews"]["action_items"][0]["resource_id"],
        review_id.to_string()
    );
    assert_eq!(
        payload["rights"]["action_items"][0]["resource_id"],
        review_id.to_string()
    );
    assert_eq!(
        payload["learning"]["action_items"][0]["resource_id"],
        project_id.to_string()
    );
    let learning_age = payload["learning"]["action_items"][0]["age_seconds"]
        .as_i64()
        .unwrap();
    assert!((7 * 86_400 + 3_590..=7 * 86_400 + 3_700).contains(&learning_age));
    assert_eq!(
        payload["workspaces"]["action_items"][0]["resource_id"],
        workspace_run_id.to_string()
    );
    let workspace_age = payload["workspaces"]["action_items"][0]["age_seconds"]
        .as_i64()
        .unwrap();
    assert!((50..=120).contains(&workspace_age));
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
    assert!(metrics.contains("alpha_content_generation_jobs{state=\"expired_lease\"} 25"));
}

async fn review_candidate(pool: &PgPool, suffix: &str, state: &str) -> (Uuid, Uuid, Vec<u8>) {
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
    let review_id: Uuid = sqlx::query_scalar("INSERT INTO content_review_items (artifact_id,artifact_hash,author_user_id,state,impact,create_idempotency_key) VALUES ($1,$2,$3,$4,'standard',$5) RETURNING id")
        .bind(artifact_id)
        .bind(&artifact_hash)
        .bind(author)
        .bind(state)
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
    (review_id, reviewer, artifact_hash)
}

async fn provenance(
    pool: &PgPool,
    review_id: Uuid,
    reviewer: Uuid,
    artifact_hash: &[u8],
    commercial_use_allowed: bool,
    redistribution_allowed: bool,
) -> (Uuid, Vec<u8>) {
    let provenance_hash = Sha256::digest(review_id.as_bytes()).to_vec();
    let provenance_id: Uuid = sqlx::query_scalar("INSERT INTO content_provenance_records (review_item_id,revision_number,artifact_hash,origin_type,creator_or_provider,source_revision,license_basis,license_identifier,attribution,modification_status,commercial_use_allowed,redistribution_allowed,evidence_reference,evidence_hash,attachment_metadata,legal_status,provenance_hash,recorded_by,idempotency_key) VALUES ($1,1,$2,'original','ALPHA author','revision-1','copyright_owner','copyright-owner','ALPHA author','unmodified',$3,$4,'internal evidence',$5,'{}','pending',$6,$7,$8) RETURNING id")
        .bind(review_id)
        .bind(artifact_hash)
        .bind(commercial_use_allowed)
        .bind(redistribution_allowed)
        .bind(vec![5_u8; 32])
        .bind(&provenance_hash)
        .bind(reviewer)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap();
    (provenance_id, provenance_hash)
}

async fn rights_receipt(
    pool: &PgPool,
    review_id: Uuid,
    reviewer: Uuid,
    provenance_id: Uuid,
    provenance_hash: &[u8],
    commercial_use_allowed: bool,
    redistribution_allowed: bool,
) {
    sqlx::query("INSERT INTO content_rights_review_receipts (review_item_id,revision_number,reviewer_user_id,provenance_id,provenance_hash,decision,basis,evidence,commercial_use_allowed,redistribution_allowed,provider_terms_version,idempotency_key,receipt_hash) VALUES ($1,1,$2,$3,$4,'approve','original','rights verified',$5,$6,'terms-v1',$7,$8)")
        .bind(review_id)
        .bind(reviewer)
        .bind(provenance_id)
        .bind(provenance_hash)
        .bind(commercial_use_allowed)
        .bind(redistribution_allowed)
        .bind(Uuid::now_v7())
        .bind(vec![9_u8; 32])
        .execute(pool)
        .await
        .unwrap();
}

async fn approve_rights(pool: &PgPool, review_id: Uuid, reviewer: Uuid, artifact_hash: &[u8]) {
    let (provenance_id, provenance_hash) =
        provenance(pool, review_id, reviewer, artifact_hash, true, true).await;
    rights_receipt(
        pool,
        review_id,
        reviewer,
        provenance_id,
        &provenance_hash,
        true,
        true,
    )
    .await;
}

fn test_database_url(pool: &PgPool) -> String {
    let options = pool.connect_options();
    let database = options.get_database().unwrap();
    let mut url = Url::from_str(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(database);
    url.to_string()
}

#[sqlx::test(migrations = "./migrations")]
async fn publication_gate_fails_closed_for_pending_flags_and_stale_revision_then_passes(
    pool: PgPool,
) {
    let settings = Settings::from_pairs(std::collections::HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let human = role_session(&app, &pool, "operations-human", "HUMAN_REVIEWER").await;
    let legal = role_session(&app, &pool, "operations-legal", "LEGAL_REVIEWER").await;

    let (approved, approved_reviewer, approved_hash) =
        review_candidate(&pool, "approved", "approved").await;
    approve_rights(&pool, approved, approved_reviewer, &approved_hash).await;
    let (missing, _, _) = review_candidate(&pool, "missing", "approved").await;

    let (pending, _, pending_hash) =
        review_candidate(&pool, "pending", "human_review_pending").await;
    assert_eq!(
        post(
            &app,
            &human,
            &format!("/api/v1/content/reviews/{pending}/human"),
            json!({"decision":"approve","note":"운영 게이트 실제 전이","idempotency_key":Uuid::now_v7()}),
        )
        .await,
        StatusCode::OK
    );
    let state: String = sqlx::query_scalar("SELECT state FROM content_review_items WHERE id=$1")
        .bind(pending)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "rights_review_pending");

    let (commercial, commercial_reviewer, commercial_hash) =
        review_candidate(&pool, "commercial", "approved").await;
    let (commercial_provenance, commercial_provenance_hash) = provenance(
        &pool,
        commercial,
        commercial_reviewer,
        &commercial_hash,
        false,
        true,
    )
    .await;
    rights_receipt(
        &pool,
        commercial,
        commercial_reviewer,
        commercial_provenance,
        &commercial_provenance_hash,
        false,
        true,
    )
    .await;

    let (redistribution, redistribution_reviewer, redistribution_hash) =
        review_candidate(&pool, "redistribution", "approved").await;
    let (redistribution_provenance, redistribution_provenance_hash) = provenance(
        &pool,
        redistribution,
        redistribution_reviewer,
        &redistribution_hash,
        true,
        false,
    )
    .await;
    rights_receipt(
        &pool,
        redistribution,
        redistribution_reviewer,
        redistribution_provenance,
        &redistribution_provenance_hash,
        true,
        false,
    )
    .await;

    let (stale, stale_reviewer, stale_hash) = review_candidate(&pool, "stale", "approved").await;
    approve_rights(&pool, stale, stale_reviewer, &stale_hash).await;
    let (artifact_id, author): (Uuid, Uuid) =
        sqlx::query_as("SELECT artifact_id,author_user_id FROM content_review_items WHERE id=$1")
            .bind(stale)
            .fetch_one(&pool)
            .await
            .unwrap();
    let stale_artifact: Uuid = sqlx::query_scalar("INSERT INTO content_artifacts (job_id,attempt_id,artifact_kind,payload,content_hash) SELECT job_id,attempt_id,'candidate','{\"title\":\"stale-v2\"}',decode(repeat('06',32),'hex') FROM content_artifacts WHERE id=$1 RETURNING id")
        .bind(artifact_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_review_revisions (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key,request_hash) VALUES ($1,2,$2,decode(repeat('06',32),'hex'),$3,$4,decode(repeat('07',32),'hex'))")
        .bind(stale)
        .bind(stale_artifact)
        .bind(author)
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE content_review_items SET revision_number=2,artifact_id=$2,artifact_hash=decode(repeat('06',32),'hex') WHERE id=$1")
        .bind(stale)
        .bind(stale_artifact)
        .execute(&pool)
        .await
        .unwrap();

    let database_url = test_database_url(&pool);

    let binary = std::env::var("CARGO_BIN_EXE_publication_gate").unwrap();
    let failure = Command::new(&binary)
        .env("DATABASE_URL", &database_url)
        .output()
        .unwrap();
    assert_eq!(failure.status.code(), Some(1));
    let stderr = String::from_utf8(failure.stderr).unwrap();
    assert!(stderr.contains(&pending.to_string()));
    assert!(stderr.contains("rights_not_approved"));
    assert!(stderr.contains(&missing.to_string()));
    assert!(stderr.contains("missing_provenance"));
    assert!(stderr.contains(&commercial.to_string()));
    assert!(stderr.contains("commercial_use_denied"));
    assert!(stderr.contains(&redistribution.to_string()));
    assert!(stderr.contains("redistribution_denied"));
    assert!(stderr.contains(&stale.to_string()));
    assert!(stderr.contains("stale_revision_evidence"));

    let (pending_provenance, pending_provenance_hash) =
        provenance(&pool, pending, legal.user_id, &pending_hash, true, true).await;
    let pending_provenance_hash = pending_provenance_hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        post(
            &app,
            &legal,
            &format!("/api/v1/content/reviews/{pending}/rights"),
            json!({"provenance_id":pending_provenance,"provenance_hash":pending_provenance_hash,"decision":"approve","basis":"original","evidence":"운영 게이트 권리 확인","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"terms-v1","idempotency_key":Uuid::now_v7()}),
        )
        .await,
        StatusCode::OK
    );
    sqlx::query("UPDATE content_review_items SET state='rejected' WHERE id=ANY($1)")
        .bind(vec![missing, commercial, redistribution, stale])
        .execute(&pool)
        .await
        .unwrap();

    let success = Command::new(binary)
        .env("DATABASE_URL", database_url)
        .output()
        .unwrap();
    assert_eq!(success.status.code(), Some(0));
}
