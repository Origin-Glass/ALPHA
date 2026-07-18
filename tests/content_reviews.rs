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
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

struct Session {
    user_id: Uuid,
    cookie: String,
    csrf: String,
}

#[sqlx::test(migrations = "./migrations")]
async fn provenance_uses_actual_attempt_provider_and_rejects_generated_lineage_downgrade(
    pool: PgPool,
) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, &pool, "fallback-author", "CONTENT_CREATOR").await;
    let (artifact_id, _) = artifact(&pool, author.user_id, "fallback provenance").await;
    let fallback:Uuid=sqlx::query_scalar("INSERT INTO content_provider_configs (name,kind,protocol,base_url,model,cost_per_generation_microunits,credential_env_var,created_by) VALUES ('actual-fallback','local','openai_compatible','http://127.0.0.1:11434','fallback-model',1,NULL,$1) RETURNING id").bind(author.user_id).fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE content_generation_attempts SET provider_snapshot=jsonb_build_object('provider_id',$2::uuid,'model','fallback-model','protocol','openai_compatible') WHERE id=(SELECT attempt_id FROM content_artifacts WHERE id=$1)").bind(artifact_id).bind(fallback).execute(&pool).await.unwrap();
    let response = post(
        &app,
        &author,
        "/api/v1/content/reviews",
        json!({"artifact_id":artifact_id,"impact":"standard","idempotency_key":Uuid::now_v7()}),
    )
    .await;
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    let id = body["id"].as_str().unwrap();
    let mut forged = provenance_payload("pending");
    forged["origin_type"] = json!("original");
    forged["license_basis"] = json!("copyright_owner");
    assert_eq!(
        post(
            &app,
            &author,
            &format!("/api/v1/content/reviews/{id}/provenance"),
            forged
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let _ = provenance(&app, &author, id, "pending").await;
    let response = get(
        &app,
        Some(&author),
        &format!("/api/v1/content/reviews/{id}"),
    )
    .await;
    let detail: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 131_072).await.unwrap()).unwrap();
    assert_eq!(detail["provenance"]["ai_provider_id"], json!(fallback));
    assert_eq!(detail["provenance"]["ai_provider_name"], "actual-fallback");
    assert_eq!(detail["provenance"]["ai_model"], "fallback-model");
    assert_ne!(
        detail["provenance"]["source_revision"],
        "immutable-artifact-v1"
    );
}

async fn login(app: &axum::Router, pool: &PgPool, handle: &str, role: &str) -> Session {
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
            json!({"version":"2026-07-18","choices":{"terms":true,"privacy":true}})
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

async fn get(app: &axum::Router, session: Option<&Session>, uri: &str) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(session) = session {
        request = request.header(header::COOKIE, &session.cookie);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn artifact(pool: &PgPool, author: Uuid, title: &str) -> (Uuid, String) {
    artifact_of_type(pool,author,title,"algorithm_problem",json!({"title":title,"statement":"두 수를 더하세요","solution":"비공개 정답","hidden_tests":["secret"],"provider_debug":"비공개"})).await
}

async fn artifact_of_type(
    pool: &PgPool,
    author: Uuid,
    title: &str,
    content_type: &str,
    payload: Value,
) -> (Uuid, String) {
    let provider: Uuid = sqlx::query_scalar("INSERT INTO content_provider_configs (name,kind,protocol,base_url,model,cost_per_generation_microunits,credential_env_var,created_by) VALUES ($1,'local','openai_compatible','http://127.0.0.1:11434','review-test',1,NULL,$2) RETURNING id")
        .bind(format!("review-{}", &Uuid::now_v7().simple().to_string()[..20])).bind(author).fetch_one(pool).await.unwrap();
    let request_hash = Sha256::digest(title.as_bytes()).to_vec();
    let job: Uuid = sqlx::query_scalar("INSERT INTO content_generation_jobs (created_by,provider_id,content_type,request_spec,request_hash,status,estimated_cost_microunits,attempt_cost_microunits,max_attempts,settled) VALUES ($1,$2,$3,$4,$5,'completed',3,1,3,true) RETURNING id")
        .bind(author).bind(provider).bind(content_type).bind(json!({"title":title})).bind(&request_hash).fetch_one(pool).await.unwrap();
    let attempt: Uuid = sqlx::query_scalar("INSERT INTO content_generation_attempts (job_id,attempt_number,provider_snapshot,request_hash,response_hash,status,completed_at) VALUES ($1,1,$2,$3,$3,'completed',now()) RETURNING id")
        .bind(job).bind(json!({"provider_id":provider,"model":"review-test","protocol":"openai_compatible"})).bind(&request_hash).fetch_one(pool).await.unwrap();
    let hash = Sha256::digest(format!("artifact:{title}").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let id: Uuid = sqlx::query_scalar("INSERT INTO content_artifacts (job_id,attempt_id,artifact_kind,payload,content_hash) VALUES ($1,$2,'candidate',$3,decode($4,'hex')) RETURNING id")
        .bind(job).bind(attempt).bind(payload).bind(&hash).fetch_one(pool).await.unwrap();
    (id, hash)
}

fn receipt(kind: &str, input_hash: &str, output_hash: &str, outcome: &str) -> Value {
    let (role, prompt) = match kind {
        "specification_pedagogy" => ("specification-pedagogy-reviewer", "spec-pedagogy-v1"),
        "solution_judge" => ("solution-judge-reviewer", "solution-judge-v1"),
        _ => ("adversarial-rights-reviewer", "adversarial-rights-v1"),
    };
    json!({"kind":kind,"provider":"review-provider","model":"review-model","prompt_version":prompt,
        "role_identifier":role,
        "review_seed":match kind {"specification_pedagogy"=>11,"solution_judge"=>22,_=>33},"idempotency_key":Uuid::now_v7(),
        "review_context":{"artifact_hash":input_hash,"policy":"독립 검토","kind":kind},
        "review_response":{"summary":format!("검토 결과 {output_hash}"),"scores":{"quality":0.9},"outcome":outcome},"latency_ms":120,"usage":{"input_tokens":40,"output_tokens":20},
        "outcome":outcome,"findings":if outcome == "pass" { json!([]) } else { json!([{"severity":"high","evidence":"statement.constraints","message":"제약 조건 누락","proposed_fix":"입력 범위를 명시하세요"}]) }})
}

async fn provenance(
    app: &axum::Router,
    session: &Session,
    id: &str,
    status: &str,
) -> (Uuid, String) {
    let response = post(
        app,
        session,
        &format!("/api/v1/content/reviews/{id}/provenance"),
        provenance_payload(status),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    (
        body["id"].as_str().unwrap().parse().unwrap(),
        body["provenance_hash"].as_str().unwrap().to_owned(),
    )
}
fn provenance_payload(status: &str) -> Value {
    json!({
        "origin_type":"ai_generated","creator_or_provider":"ALPHA content factory","source_url":null,"source_revision":"immutable-artifact-v1",
        "license_basis":"provider_contract","license_identifier":"provider-terms-2026-07","attribution":"AI 생성 및 사람 검토",
        "modification_status":"unmodified","commercial_use_allowed":true,"redistribution_allowed":true,
        "evidence_reference":"audit://generation-job","evidence_hash":"91".repeat(32),"attachment_metadata":{"kind":"generation_receipt"},
        "legal_status":status,"idempotency_key":Uuid::now_v7()
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn exact_three_reviews_and_separated_human_rights_pilot_gate_publication(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, &pool, "review-author", "CONTENT_CREATOR").await;
    let ai = login(&app, &pool, "ai-reviewer", "AI_CONTENT_OPERATOR").await;
    let human = login(&app, &pool, "human-reviewer", "HUMAN_REVIEWER").await;
    let rights = login(&app, &pool, "legal-reviewer", "LEGAL_REVIEWER").await;
    let publisher = login(&app, &pool, "publisher", "ADMIN").await;
    let (artifact_id, hash) = artifact(&pool, author.user_id, "불변 검토 대상").await;

    let response = post(
        &app,
        &author,
        "/api/v1/content/reviews",
        json!({"artifact_id":artifact_id,"impact":"high","idempotency_key":Uuid::now_v7()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    let id = body["id"].as_str().unwrap();

    for (kind, output) in [
        ("specification_pedagogy", "11".repeat(32)),
        ("solution_judge", "22".repeat(32)),
        ("adversarial_rights", "33".repeat(32)),
    ] {
        assert_eq!(
            post(
                &app,
                &ai,
                &format!("/api/v1/content/reviews/{id}/ai-receipts"),
                receipt(kind, &hash, &output, "pass")
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        post(
            &app,
            &author,
            &format!("/api/v1/content/reviews/{id}/human"),
            json!({"decision":"approve","note":"완전성 검토 완료","idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let human_key = Uuid::now_v7();
    assert_eq!(
        post(
            &app,
            &human,
            &format!("/api/v1/content/reviews/{id}/human"),
            json!({"decision":"approve","note":"완전성 검토 완료","idempotency_key":human_key})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &human,
            &format!("/api/v1/content/reviews/{id}/human"),
            json!({"decision":"approve","note":"다른 근거 재사용","idempotency_key":human_key})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(post(&app, &human, &format!("/api/v1/content/reviews/{id}/rights"), json!({"provenance_id":Uuid::now_v7(),"provenance_hash":"92".repeat(32),"basis":"contract","evidence":"원본 생성 작업과 감사 추적 확인","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"2026-07","decision":"approve","idempotency_key":Uuid::now_v7()})).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(post(&app, &rights, &format!("/api/v1/content/reviews/{id}/rights"), json!({"provenance_id":Uuid::now_v7(),"provenance_hash":"92".repeat(32),"basis":"contract","evidence":"원본 생성 작업과 감사 추적 확인","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"2026-07","decision":"approve","idempotency_key":Uuid::now_v7()})).await.status(), StatusCode::CONFLICT);
    for prohibited in ["approved", "rejected", "restricted", "removal_required"] {
        assert_eq!(
            post(
                &app,
                &author,
                &format!("/api/v1/content/reviews/{id}/provenance"),
                provenance_payload(prohibited)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut incomplete = provenance_payload("pending");
    incomplete["origin_type"] = json!("licensed");
    assert_eq!(
        post(
            &app,
            &author,
            &format!("/api/v1/content/reviews/{id}/provenance"),
            incomplete
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let (provenance_id, provenance_hash) = provenance(&app, &author, id, "pending").await;
    assert_eq!(post(&app,&rights,&format!("/api/v1/content/reviews/{id}/rights"),json!({"provenance_id":provenance_id,"provenance_hash":"93".repeat(32),"basis":"contract","evidence":"원본 생성 작업과 감사 추적 확인","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"2026-07","decision":"approve","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
    let rights_key = Uuid::now_v7();
    assert_eq!(post(&app, &rights, &format!("/api/v1/content/reviews/{id}/rights"), json!({"provenance_id":provenance_id,"provenance_hash":provenance_hash,"basis":"contract","evidence":"원본 생성 작업과 감사 추적 확인","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"2026-07","decision":"approve","idempotency_key":rights_key})).await.status(), StatusCode::OK);
    assert_eq!(post(&app, &rights, &format!("/api/v1/content/reviews/{id}/rights"), json!({"provenance_id":provenance_id,"provenance_hash":provenance_hash,"basis":"contract","evidence":"변조된 근거","commercial_use_allowed":true,"redistribution_allowed":true,"provider_terms_version":"2026-07","decision":"approve","idempotency_key":rights_key})).await.status(), StatusCode::CONFLICT);
    let pilot_key = Uuid::now_v7();
    assert_eq!(post(&app, &human, &format!("/api/v1/content/reviews/{id}/pilot"), json!({"cohort":"내부 베타 20명","source_reference":"pilot://run-2026-07","started_at":"2026-07-18T00:00:00Z","ended_at":"2026-07-19T00:00:00Z","participants":20,"completion_rate":0.8,"failure_rate":0.1,"report_count":0,"rollback_ready":true,"rollback_evidence":"게시 해제 절차와 담당자 확인","decision":"pass","note":"기준 충족","idempotency_key":pilot_key})).await.status(), StatusCode::OK);
    assert_eq!(post(&app, &human, &format!("/api/v1/content/reviews/{id}/pilot"), json!({"cohort":"변조 코호트","source_reference":"pilot://run-2026-07","started_at":"2026-07-18T00:00:00Z","ended_at":"2026-07-19T00:00:00Z","participants":20,"completion_rate":0.8,"failure_rate":0.1,"report_count":0,"rollback_ready":true,"rollback_evidence":"게시 해제 절차와 담당자 확인","decision":"pass","note":"기준 충족","idempotency_key":pilot_key})).await.status(), StatusCode::CONFLICT);
    assert_eq!(
        post(
            &app,
            &publisher,
            &format!("/api/v1/content/reviews/{id}/publish"),
            json!({"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&app, None, &format!("/api/v1/content/public/{id}"))
            .await
            .status(),
        StatusCode::OK
    );
    let publication_evidence: Value = sqlx::query_scalar(
        "SELECT metadata->'publication_evidence' FROM audit_events WHERE action='content.published' AND target_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(publication_evidence["revision"], 1);
    assert_eq!(publication_evidence["artifact_hash"], hash);
    assert_eq!(
        publication_evidence["provenance_id"],
        provenance_id.to_string()
    );
    assert_eq!(
        publication_evidence["ai_receipt_ids"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(publication_evidence["human_receipt_id"].is_string());
    assert!(publication_evidence["rights_receipt_id"].is_string());
    assert!(publication_evidence["pilot_receipt_id"].is_string());
    assert_eq!(
        post(
            &app,
            &publisher,
            &format!("/api/v1/content/reviews/{id}/removal"),
            json!({"reason":"권리 제거 요청 접수","idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&app, None, &format!("/api/v1/content/public/{id}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(post(&app,&publisher,&format!("/api/v1/content/reviews/{id}/removal/complete"),json!({"reason":"배포본 제거 완료","evidence":"CDN 및 공개 카탈로그 조회 404 확인","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::OK);
    let removal_evidence: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_events WHERE action='content.removal_completed' AND target_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(removal_evidence["reason"], "배포본 제거 완료");
    assert_eq!(
        removal_evidence["evidence"],
        "CDN 및 공개 카탈로그 조회 404 확인"
    );
    assert_eq!(removal_evidence["payload_hash"].as_str().unwrap().len(), 64);
    let (replacement, _) = artifact(&pool, author.user_id, "제거 후 대체 리비전").await;
    let replaced = post(
        &app,
        &author,
        &format!("/api/v1/content/reviews/{id}/revise"),
        json!({"artifact_id":replacement,"idempotency_key":Uuid::now_v7()}),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(replaced.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(body["state"], "ai_review_pending");
    assert_eq!(body["revision"], 2);
    let detail_response = get(
        &app,
        Some(&author),
        &format!("/api/v1/content/reviews/{id}"),
    )
    .await;
    let detail: Value =
        serde_json::from_slice(&to_bytes(detail_response.into_body(), 65_536).await.unwrap())
            .unwrap();
    assert_eq!(detail["state"], "ai_review_pending");
    assert_eq!(detail["revision"], 2);
    assert!(detail["ai_receipts"].as_array().unwrap().is_empty());
    assert!(detail["human_receipts"].as_array().unwrap().is_empty());
    assert!(detail["rights_receipts"].as_array().unwrap().is_empty());
    assert!(detail["pilot_receipts"].as_array().unwrap().is_empty());
    assert!(detail["provenance"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn changed_artifact_restarts_all_reviews_and_rejects_stale_or_reused_receipts(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, &pool, "revision-author", "CONTENT_CREATOR").await;
    let ai = login(&app, &pool, "revision-ai", "AI_CONTENT_OPERATOR").await;
    let human = login(&app, &pool, "revision-human", "HUMAN_REVIEWER").await;
    let (artifact_v1, hash_v1) = artifact(&pool, author.user_id, "첫 리비전").await;
    let create = post(
        &app,
        &author,
        "/api/v1/content/reviews",
        json!({"artifact_id":artifact_v1,"impact":"standard","idempotency_key":Uuid::now_v7()}),
    )
    .await;
    let body: Value =
        serde_json::from_slice(&to_bytes(create.into_body(), 16_384).await.unwrap()).unwrap();
    let id = body["id"].as_str().unwrap();

    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("specification_pedagogy", &hash_v1, &"71".repeat(32), "fail")
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (artifact_v2, hash_v2) = artifact(&pool, author.user_id, "두 번째 리비전").await;
    assert_eq!(
        post(
            &app,
            &author,
            &format!("/api/v1/content/reviews/{id}/revise"),
            json!({"artifact_id":artifact_v2,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("specification_pedagogy", &hash_v1, &"72".repeat(32), "pass")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let shared = "73".repeat(32);
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("specification_pedagogy", &hash_v2, &shared, "pass")
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("solution_judge", &hash_v2, &shared, "pass")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("solution_judge", &hash_v2, &"74".repeat(32), "pass")
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(post(&app,&human,&format!("/api/v1/content/reviews/{id}/human"),json!({"decision":"approve","note":"두 개만 있는 상태","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("adversarial_rights", &hash_v2, &"75".repeat(32), "pass")
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(post(&app,&human,&format!("/api/v1/content/reviews/{id}/human"),json!({"decision":"changes_requested","note":"예시를 더 명확히 수정","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::OK);
    let (artifact_v3, hash_v3) = artifact(&pool, author.user_id, "세 번째 리비전").await;
    assert_eq!(
        post(
            &app,
            &author,
            &format!("/api/v1/content/reviews/{id}/revise"),
            json!({"artifact_id":artifact_v3,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            receipt("specification_pedagogy", &hash_v3, &"76".repeat(32), "pass")
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(post(&app,&human,&format!("/api/v1/content/reviews/{id}/human"),json!({"decision":"approve","note":"이전 리비전 승인 재사용 시도","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn review_endpoints_enforce_csrf_owner_scope_and_atomic_idempotency(pool: PgPool) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, &pool, "scope-author", "CONTENT_CREATOR").await;
    let intruder = login(&app, &pool, "scope-intruder", "CONTENT_CREATOR").await;
    let ai = login(&app, &pool, "scope-ai", "AI_CONTENT_OPERATOR").await;
    let (artifact_id, hash) = artifact(&pool, author.user_id, "소유권 대상").await;
    let key = Uuid::now_v7();
    let body = json!({"artifact_id":artifact_id,"impact":"standard","idempotency_key":key});
    let (first, second) = tokio::join!(
        post(&app, &author, "/api/v1/content/reviews", body.clone()),
        post(&app, &author, "/api/v1/content/reviews", body)
    );
    assert!(matches!(
        first.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    assert!(matches!(
        second.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    let first_body: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 16_384).await.unwrap()).unwrap();
    let second_body: Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(first_body["id"], second_body["id"]);
    let id = first_body["id"].as_str().unwrap();
    assert_eq!(
        get(
            &app,
            Some(&intruder),
            &format!("/api/v1/content/reviews/{id}")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &intruder,
            "/api/v1/content/reviews",
            json!({"artifact_id":artifact_id,"impact":"standard","idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let no_csrf = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/content/reviews/{id}/ai-receipts"))
                .header(header::COOKIE, &ai.cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    receipt("specification_pedagogy", &hash, &"81".repeat(32), "pass").to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
    let mut unsafe_receipt = receipt("specification_pedagogy", &hash, &"80".repeat(32), "pass");
    unsafe_receipt["review_response"]["raw_response"] = json!({"reasoning":"secret chain"});
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            unsafe_receipt
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let idem = Uuid::now_v7();
    let mut valid = receipt("specification_pedagogy", &hash, &"82".repeat(32), "pass");
    valid["idempotency_key"] = json!(idem);
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            valid.clone()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            valid.clone()
        )
        .await
        .status(),
        StatusCode::OK
    );
    valid["review_response"]["summary"] = json!(format!("변경 결과 {}", "83".repeat(32)));
    assert_eq!(
        post(
            &app,
            &ai,
            &format!("/api/v1/content/reviews/{id}/ai-receipts"),
            valid
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM content_ai_review_receipts WHERE review_item_id=$1",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let detail = get(&app, Some(&ai), &format!("/api/v1/content/reviews/{id}")).await;
    let detail_body = String::from_utf8(
        to_bytes(detail.into_body(), 131_072)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!detail_body.contains("raw_response"));
    assert!(!detail_body.contains("secret chain"));
    assert_eq!(
        post(
            &app,
            &author,
            "/api/v1/content/reviews",
            json!({"artifact_id":artifact_id,"impact":"high","idempotency_key":key})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn public_projection_exposes_each_learning_body_but_never_answers_or_provider_data(
    pool: PgPool,
) {
    let settings = Settings::from_pairs(HashMap::from([
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgres://test"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]))
    .unwrap();
    let app = router(AppState::new(pool.clone(), settings));
    let author = login(&app, &pool, "projection-author", "CONTENT_CREATOR").await;
    let cases = [
        (
            "algorithm_problem",
            json!({"title":"알고리즘","statement":"문제 본문","examples":[{"input":"1","output":"1"}],"solution":"secret","provider_debug":"secret"}),
            "statement",
        ),
        (
            "code_reading",
            json!({"title":"코드 읽기","code":"print(1)","question":"출력은?","answer":"secret","provider_debug":"secret"}),
            "code",
        ),
        (
            "debugging",
            json!({"title":"디버깅","buggy_code":"panic!()","question":"오류를 찾으세요","fixed_code":"secret","explanation":"secret"}),
            "buggy_code",
        ),
        (
            "documentation_lesson",
            json!({"title":"문서 학습","lesson":"공식 문서를 읽습니다","learning_objective":"문서 탐색","answer":"secret","provider":"secret"}),
            "lesson",
        ),
        (
            "implementation_task",
            json!({"title":"구현","requirements":["API 구현"],"reference_solution":"secret","hidden_tests":["secret"]}),
            "requirements",
        ),
    ];
    for (kind, payload, required) in cases {
        let (artifact_id, _) = artifact_of_type(&pool, author.user_id, kind, kind, payload).await;
        let response = post(
            &app,
            &author,
            "/api/v1/content/reviews",
            json!({"artifact_id":artifact_id,"impact":"standard","idempotency_key":Uuid::now_v7()}),
        )
        .await;
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
        let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
        sqlx::query(
            "UPDATE content_review_items SET state='published',published_at=now() WHERE id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        let response = get(&app, None, &format!("/api/v1/content/public/{id}")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let public: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
        assert!(
            !public["content"][required].is_null(),
            "{kind} 학습 본문 누락"
        );
        let encoded = public.to_string();
        for forbidden in [
            "solution",
            "answer",
            "fixed_code",
            "reference_solution",
            "hidden_tests",
            "provider",
            "explanation",
        ] {
            assert!(!encoded.contains(forbidden), "{kind}에서 {forbidden} 노출");
        }
    }
}
