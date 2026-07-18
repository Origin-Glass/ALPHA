use alpha::{
    config::Settings,
    http::{AppState, router},
    understanding::{UnderstandingEvidence, evaluate_understanding},
    workspaces::{WorkspaceFileInput, validate_files},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use tower::ServiceExt;
use uuid::Uuid;

const DECLARED_IMAGE: &str =
    "alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de";
const EXECUTION_IMAGE: &str =
    "sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de";

#[test]
fn workspace_files_fail_closed_before_execution() {
    let valid = vec![
        WorkspaceFileInput::new("src/main.rs", "mod helper; fn main(){helper::run()}", false),
        WorkspaceFileInput::new("src/helper.rs", "pub fn run(){}", false),
    ];
    assert!(validate_files(&valid).is_ok());

    for files in [
        vec![WorkspaceFileInput::new("../secret", "x", false)],
        vec![WorkspaceFileInput::new("/etc/passwd", "x", false)],
        vec![WorkspaceFileInput::new("link", "x", true)],
        vec![
            WorkspaceFileInput::new("Readme.md", "a", false),
            WorkspaceFileInput::new("README.md", "b", false),
        ],
        vec![WorkspaceFileInput::new("bad\0name", "x", false)],
        vec![WorkspaceFileInput::new("src//main.rs", "x", false)],
        vec![WorkspaceFileInput::new("src/", "x", false)],
        vec![WorkspaceFileInput::new("src\\main.rs", "x", false)],
        vec![WorkspaceFileInput::new("소스/main.rs", "x", false)],
    ] {
        assert!(validate_files(&files).is_err(), "위험한 파일 목록이 허용됨");
    }

    let too_many = (0..65)
        .map(|i| WorkspaceFileInput::new(format!("src/{i}.rs"), "x", false))
        .collect::<Vec<_>>();
    assert!(validate_files(&too_many).is_err());
    assert!(
        validate_files(&[WorkspaceFileInput::new(
            "huge.txt",
            "x".repeat(262_145),
            false,
        )])
        .is_err()
    );
}

#[test]
fn copied_output_or_ai_receipt_cannot_grant_understanding() {
    let artifact_hash = [7_u8; 32];
    let original = UnderstandingEvidence {
        artifact_hash,
        explanation: "출력은 42입니다".into(),
        expected_concepts: vec!["상태 전이".into(), "오류 처리".into()],
        modification_run_hash: artifact_hash,
        transfer_run_hash: artifact_hash,
        hidden_checks_passed: true,
        ai_assessed: false,
    };
    assert!(evaluate_understanding(&original).is_err());

    let ai_only = UnderstandingEvidence {
        modification_run_hash: [8_u8; 32],
        transfer_run_hash: [9_u8; 32],
        ai_assessed: true,
        ..original
    };
    assert!(evaluate_understanding(&ai_only).is_err());

    let verified = UnderstandingEvidence {
        explanation: "상태 전이는 입력을 검증한 뒤 오류를 반환하며 오류 처리 불변식을 유지합니다"
            .into(),
        ai_assessed: false,
        ..ai_only
    };
    assert_eq!(
        evaluate_understanding(&verified).unwrap(),
        "transfer_verified"
    );
}

struct Session {
    user_id: Uuid,
    cookie: String,
    csrf: String,
}
fn app(pool: PgPool) -> axum::Router {
    router(AppState::new(
        pool,
        Settings::from_pairs(HashMap::from([
            ("APP_ENV", "test"),
            ("DATABASE_URL", "postgres://test"),
            ("TEST_IDENTITY_ENABLED", "true"),
        ]))
        .unwrap(),
    ))
}
async fn session(app: &axum::Router, handle: &str) -> Session {
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
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("alpha_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    let s = Session {
        user_id: body["user"]["id"].as_str().unwrap().parse().unwrap(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().into(),
    };
    assert_eq!(
        post(
            app,
            &s,
            "/api/v1/auth/terms",
            json!({"version":"2026-07-18","choices":{"terms":true,"privacy":true}})
        )
        .await
        .status(),
        StatusCode::OK
    );
    s
}
async fn post(app: &axum::Router, s: &Session, uri: &str, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(header::COOKIE, &s.cookie)
                .header("x-csrf-token", &s.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn put(app: &axum::Router, s: &Session, uri: &str, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::put(uri)
                .header(header::COOKIE, &s.cookie)
                .header("x-csrf-token", &s.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn get(app: &axum::Router, s: &Session, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
                .header(header::COOKIE, &s.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 262144).await.unwrap()).unwrap()
}
fn files(version: &str) -> Value {
    json!([{"path":"main.rs","content":format!("mod helper; fn main(){{println!(\"{{}}\",helper::value())}} // {version}"),"symlink":false},{"path":"helper.rs","content":format!("pub fn value()->i32{{{}}}",version),"symlink":false}])
}
async fn finish_run(pool: &PgPool, id: Uuid, stdout: &str) {
    let lease = alpha::workspaces::lease_next(
        pool,
        "test-workspace-worker",
        DECLARED_IMAGE,
        EXECUTION_IMAGE,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(lease.id, id);
    assert!(
        alpha::workspaces::mark_running(pool, id, lease.lease_token)
            .await
            .unwrap()
    );
    let outcome = alpha::sandbox::WorkspaceOutcome {
        status: "succeeded",
        exit_code: Some(0),
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
        output_truncated: false,
    };
    assert!(
        alpha::workspaces::complete_run(
            pool,
            id,
            lease.lease_token,
            &outcome,
            b"alpha-local-workspace-receipt-secret-32"
        )
        .await
        .unwrap()
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn owned_workspace_run_understanding_and_portfolio_receipts_are_exact(pool: PgPool) {
    let app = app(pool.clone());
    let owner = session(&app, "workspace-owner").await;
    let other = session(&app, "workspace-other").await;
    let created=post(&app,&owner,"/api/v1/workspaces",json!({"title":"다중 파일 CLI","template_slug":"python-test-v1","project_id":null,"files":files("41"),"idempotency_key":Uuid::now_v7()})).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let workspace = body(created).await;
    let wid = workspace["id"].as_str().unwrap();
    assert_eq!(
        get(&app, &other, &format!("/api/v1/workspaces/{wid}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let key = Uuid::now_v7();
    let uri = format!("/api/v1/workspaces/{wid}");
    let payload = json!({"files":files("42"),"expected_version":1,"idempotency_key":key});
    let (a, b) = tokio::join!(
        put(&app, &owner, &uri, payload.clone()),
        put(&app, &owner, &uri, payload)
    );
    assert!(matches!(a.status(), StatusCode::OK | StatusCode::CONFLICT));
    assert!(matches!(b.status(), StatusCode::OK | StatusCode::CONFLICT));
    assert!(a.status() == StatusCode::OK || b.status() == StatusCode::OK);
    let version: i32 = sqlx::query_scalar("SELECT version FROM project_workspaces WHERE id=$1")
        .bind(wid.parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 2);
    let mut runs = Vec::new();
    let run = post(
        &app,
        &owner,
        &format!("/api/v1/workspaces/{wid}/runs"),
        json!({"idempotency_key":Uuid::now_v7(),"expected_version":2,"validation_kind":"baseline","challenge_id":null}),
    )
    .await;
    assert_eq!(run.status(), StatusCode::CREATED);
    let id = body(run).await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    finish_run(&pool, id, "42").await;
    runs.push(id);
    let challenge=post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":wid,"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await;
    assert_eq!(challenge.status(), StatusCode::CREATED);
    let challenge = body(challenge).await;
    let resumed=body(post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":wid,"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(resumed["id"],challenge["id"]);
    assert_eq!(resumed["predictions_committed"],false);
    assert_eq!(post(&app,&owner,&format!("/api/v1/understanding/challenges/{}/predictions",challenge["id"].as_str().unwrap()),json!({"modification_prediction":"43","transfer_prediction":"44","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CREATED);
    let remounted=body(post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":wid,"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(remounted["id"],challenge["id"]);
    assert_eq!(remounted["predictions_committed"],true);
    let same=body(post(&app,&owner,"/api/v1/workspaces",json!({"title":"같은 템플릿","template_slug":"python-test-v1","project_id":null,"files":files("99"),"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(post(&app,&owner,&format!("/api/v1/workspaces/{}/runs",same["id"].as_str().unwrap()),json!({"idempotency_key":Uuid::now_v7(),"expected_version":1,"validation_kind":"transfer","challenge_id":challenge["id"]})).await.status(),StatusCode::BAD_REQUEST);
    let current: i32 = sqlx::query_scalar("SELECT version FROM project_workspaces WHERE id=$1")
        .bind(wid.parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let comment_only = json!([{"path":"main.rs","content":"mod helper; fn main(){println!(\"{}\",helper::value())} // comment-only","symlink":false},{"path":"helper.rs","content":"pub fn value()->i32{42}","symlink":false}]);
    assert_eq!(put(&app,&owner,&uri,json!({"files":comment_only,"expected_version":current,"idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::OK);
    let bad=body(post(&app,&owner,&format!("/api/v1/workspaces/{wid}/runs"),json!({"idempotency_key":Uuid::now_v7(),"expected_version":3,"validation_kind":"modification","challenge_id":challenge["id"]})).await).await;
    let bad_mod = bad["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    finish_run(&pool, bad_mod, "999").await;
    let current: i32 = sqlx::query_scalar("SELECT version FROM project_workspaces WHERE id=$1")
        .bind(wid.parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        put(
            &app,
            &owner,
            &uri,
            json!({"files":files("43"),"expected_version":current,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let run=post(&app,&owner,&format!("/api/v1/workspaces/{wid}/runs"),json!({"idempotency_key":Uuid::now_v7(),"expected_version":4,"validation_kind":"modification","challenge_id":challenge["id"]})).await;
    assert_eq!(run.status(), StatusCode::CREATED);
    let id = body(run).await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    finish_run(&pool, id, "43").await;
    runs.push(id);
    let transfer=body(post(&app,&owner,"/api/v1/workspaces",json!({"title":"다른 맥락 Python CLI","template_slug":"python-cli-v1","project_id":null,"files":[{"path":"main.py","content":"import helper; print(helper.value())","symlink":false},{"path":"helper.py","content":"def value(): return 44","symlink":false}],"idempotency_key":Uuid::now_v7()})).await).await;
    let transfer_id = transfer["id"].as_str().unwrap();
    let run=post(&app,&owner,&format!("/api/v1/workspaces/{transfer_id}/runs"),json!({"idempotency_key":Uuid::now_v7(),"expected_version":1,"validation_kind":"transfer","challenge_id":challenge["id"]})).await;
    assert_eq!(run.status(), StatusCode::CREATED);
    let id = body(run).await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    finish_run(&pool, id, "44").await;
    runs.push(id);
    assert!(
        sqlx::query(
            "UPDATE workspace_templates SET supports_tests=true WHERE slug='python-cli-v1'"
        )
        .execute(&pool)
        .await
        .is_err()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT supports_tests_snapshot FROM workspace_runs WHERE id=$1"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    assert_eq!(
        get(&app, &other, &format!("/api/v1/workspace-runs/{}", runs[0]))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let submit_uri = format!(
        "/api/v1/understanding/challenges/{}/submit",
        challenge["id"].as_str().unwrap()
    );
    assert_eq!(post(&app,&owner,&submit_uri,json!({"explanation":"main.rs에서 helper.rs 값을 읽고 상태 전이 뒤 오류 처리를 유지합니다","modification":"주석만 변경했습니다","modification_run_id":bad_mod,"transfer_run_id":runs[2],"idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::BAD_REQUEST);
    assert_eq!(post(&app,&owner,&submit_uri,json!({"explanation":"main.rs에서 helper.rs 값을 읽고 상태 전이 뒤 오류 처리를 유지합니다","modification":"opaque filler","modification_run_id":runs[1],"transfer_run_id":runs[2],"idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::BAD_REQUEST);
    let submit=post(&app,&owner,&submit_uri,json!({"explanation":"main.rs에서 helper.rs 값을 읽고 상태 전이 뒤 오류 처리를 유지합니다","modification":"helper.rs 값을 독립 변경했습니다","modification_run_id":runs[1],"transfer_run_id":runs[2],"idempotency_key":Uuid::now_v7()})).await;
    assert_eq!(submit.status(), StatusCode::OK);
    assert_eq!(body(submit).await["state"], "transfer_verified");
    sqlx::query("INSERT INTO user_roles(user_id,role) VALUES($1,'RIGHTS_REVIEWER')")
        .bind(other.user_id)
        .execute(&pool)
        .await
        .unwrap();
    let rights_key = Uuid::now_v7();
    let rights_payload = json!({"source_run_id":runs[2],"provenance":"original","license_identifier":"Copyright-owner","idempotency_key":rights_key});
    let (rights_a, rights_b) = tokio::join!(
        post(
            &app,
            &other,
            "/api/v1/portfolio/rights",
            rights_payload.clone()
        ),
        post(&app, &other, "/api/v1/portfolio/rights", rights_payload)
    );
    assert!(matches!(
        rights_a.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    assert!(matches!(
        rights_b.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    let rights_a = body(rights_a).await;
    let rights_b = body(rights_b).await;
    assert_eq!(rights_a["id"], rights_b["id"]);
    let rights = rights_a["id"].clone();
    assert_eq!(post(&app,&other,"/api/v1/portfolio/rights",json!({"source_run_id":runs[2],"provenance":"original","license_identifier":"다른 라이선스","idempotency_key":rights_key})).await.status(),StatusCode::CONFLICT);
    let portfolio_key = Uuid::now_v7();
    let portfolio_payload = json!({"title":"학습 CLI","problem":"학습 기록 계산","target_user":"학습자","workspace_id":transfer_id,"source_run_id":runs[2],"rights_receipt_id":rights,"visibility":"public","assistance_disclosure":"문서 탐색 도움 사용","idempotency_key":portfolio_key});
    let (draft_a, draft_b) = tokio::join!(
        post(&app, &owner, "/api/v1/portfolio", portfolio_payload.clone()),
        post(&app, &owner, "/api/v1/portfolio", portfolio_payload)
    );
    assert!(matches!(
        draft_a.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    assert!(matches!(
        draft_b.status(),
        StatusCode::CREATED | StatusCode::OK
    ));
    let draft = body(draft_a).await;
    assert_eq!(draft["id"], body(draft_b).await["id"]);
    assert_eq!(post(&app,&owner,"/api/v1/portfolio",json!({"title":"다른 제목","problem":"학습 기록 계산","target_user":"학습자","workspace_id":transfer_id,"source_run_id":runs[2],"rights_receipt_id":rights,"visibility":"public","assistance_disclosure":"문서 탐색 도움 사용","idempotency_key":portfolio_key})).await.status(),StatusCode::CONFLICT);
    assert!(
        !draft["evidence_labels"]
            .as_array()
            .unwrap()
            .contains(&json!("TESTED"))
    );
    let pid = draft["id"].as_str().unwrap();
    assert_eq!(
        get(&app, &other, &format!("/api/v1/portfolio/{pid}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &owner,
            &format!("/api/v1/portfolio/{pid}/publish"),
            json!({"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let public = get(&app, &other, &format!("/api/v1/portfolio/public/{pid}")).await;
    assert_eq!(public.status(), StatusCode::OK);
    let public = body(public).await;
    assert!(public.get("workspace_id").is_none());
    assert!(
        public["assistance_disclosure"]
            .as_str()
            .unwrap()
            .contains("학습자 공개")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn workspace_lease_cancel_retry_and_stale_completion_are_fenced(pool: PgPool) {
    let app = app(pool.clone());
    let owner = session(&app, "lease-owner").await;
    let created=body(post(&app,&owner,"/api/v1/workspaces",json!({"title":"임대 검증","template_slug":"python-cli-v1","project_id":null,"files":files("42"),"idempotency_key":Uuid::now_v7()})).await).await;
    let wid = created["id"].as_str().unwrap();
    let key = Uuid::now_v7();
    let uri = format!("/api/v1/workspaces/{wid}/runs");
    let (a, b) = tokio::join!(
        post(
            &app,
            &owner,
            &uri,
            json!({"idempotency_key":key,"expected_version":1})
        ),
        post(
            &app,
            &owner,
            &uri,
            json!({"idempotency_key":key,"expected_version":1})
        )
    );
    assert!(matches!(a.status(), StatusCode::CREATED | StatusCode::OK));
    assert!(matches!(b.status(), StatusCode::CREATED | StatusCode::OK));
    let aid = body(a).await["id"].clone();
    let bid = body(b).await["id"].clone();
    assert_eq!(aid, bid);
    let id = aid.as_str().unwrap().parse::<Uuid>().unwrap();
    assert!(
        alpha::workspaces::lease_next(
            &pool,
            "wrong-image-worker",
            "other-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de",
            EXECUTION_IMAGE
        )
        .await
        .unwrap()
        .is_none()
    );
    let first = alpha::workspaces::lease_next(&pool, "worker-a", DECLARED_IMAGE, EXECUTION_IMAGE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.id, id);
    assert!(
        alpha::workspaces::mark_running(&pool, id, first.lease_token)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE workspace_runs SET lease_expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let second = alpha::workspaces::lease_next(&pool, "worker-b", DECLARED_IMAGE, EXECUTION_IMAGE)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.lease_token, second.lease_token);
    assert!(
        alpha::workspaces::mark_running(&pool, id, second.lease_token)
            .await
            .unwrap()
    );
    let outcome = alpha::sandbox::WorkspaceOutcome {
        status: "succeeded",
        exit_code: Some(0),
        stdout: "stale".into(),
        stderr: String::new(),
        output_truncated: false,
    };
    assert!(
        !alpha::workspaces::complete_run(
            &pool,
            id,
            first.lease_token,
            &outcome,
            b"alpha-local-workspace-receipt-secret-32"
        )
        .await
        .unwrap()
    );
    let cancel_key = Uuid::now_v7();
    let cancel_uri = format!("/api/v1/workspace-runs/{id}/cancel");
    let cancel_body = json!({"idempotency_key":cancel_key});
    assert_eq!(
        post(&app, &owner, &cancel_uri, cancel_body.clone())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(&app, &owner, &cancel_uri, cancel_body).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &owner,
            &cancel_uri,
            json!({"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert!(
        alpha::workspaces::run_cancelled(&pool, id, second.lease_token)
            .await
            .unwrap()
    );
    assert!(
        !alpha::workspaces::complete_run(
            &pool,
            id,
            second.lease_token,
            &outcome,
            b"alpha-local-workspace-receipt-secret-32"
        )
        .await
        .unwrap()
    );
    alpha::workspaces::finish_cancel(&pool, id, second.lease_token)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM workspace_runs WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");

    let leased_cancel = body(
        post(
            &app,
            &owner,
            &uri,
            json!({"idempotency_key":Uuid::now_v7(),"expected_version":1}),
        )
        .await,
    )
    .await;
    let leased_cancel_id = leased_cancel["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let leased = alpha::workspaces::lease_next(
        &pool,
        "worker-before-start",
        DECLARED_IMAGE,
        EXECUTION_IMAGE,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(leased.id, leased_cancel_id);
    assert_eq!(
        post(
            &app,
            &owner,
            &format!("/api/v1/workspace-runs/{leased_cancel_id}/cancel"),
            json!({"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(
        !alpha::workspaces::mark_running(&pool, leased_cancel_id, leased.lease_token)
            .await
            .unwrap()
    );
    alpha::workspaces::finish_cancel(&pool, leased_cancel_id, leased.lease_token)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workspace_runs WHERE id=$1")
            .bind(leased_cancel_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "cancelled"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn forged_success_and_self_rights_approval_do_not_create_evidence(pool: PgPool) {
    let app = app(pool.clone());
    let owner = session(&app, "evidence-owner").await;
    let workspace=body(post(&app,&owner,"/api/v1/workspaces",json!({"title":"위조 방지","template_slug":"python-cli-v1","project_id":null,"files":files("42"),"idempotency_key":Uuid::now_v7()})).await).await;
    let run = body(
        post(
            &app,
            &owner,
            &format!(
                "/api/v1/workspaces/{}/runs",
                workspace["id"].as_str().unwrap()
            ),
            json!({"idempotency_key":Uuid::now_v7(),"expected_version":1}),
        )
        .await,
    )
    .await;
    let run_id = run["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert!(sqlx::query("UPDATE workspace_runs SET status='succeeded',exit_code=0,stdout='42',stdout_hash=$2,runner_receipt_hash=$2,runner_receipt_mac=$2,completed_at=now() WHERE id=$1")
        .bind(run_id).bind(vec![9_u8;32]).execute(&pool).await.is_err());
    assert_eq!(post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":workspace["id"],"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
    assert_eq!(
        post(
            &app,
            &owner,
            &format!("/api/v1/workspace-runs/{run_id}/cancel"),
            json!({"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let forged_running = body(
        post(
            &app,
            &owner,
            &format!(
                "/api/v1/workspaces/{}/runs",
                workspace["id"].as_str().unwrap()
            ),
            json!({"idempotency_key":Uuid::now_v7(),"expected_version":1}),
        )
        .await,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let lease =
        alpha::workspaces::lease_next(&pool, "forgery-worker", DECLARED_IMAGE, EXECUTION_IMAGE)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(lease.id, forged_running);
    assert!(
        alpha::workspaces::mark_running(&pool, forged_running, lease.lease_token)
            .await
            .unwrap()
    );
    assert!(
        sqlx::query("UPDATE workspace_runs SET artifact='[]' WHERE id=$1")
            .bind(forged_running)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE workspace_runs SET status='succeeded',lease_token=NULL,leased_by=NULL,lease_expires_at=NULL,exit_code=0,stdout='42',stdout_hash=$2,deterministic_checks_passed=true,runner_receipt_hash=$2,runner_receipt_mac=$2,completed_at=now() WHERE id=$1").bind(forged_running).bind(vec![7_u8;32]).execute(&pool).await.unwrap();
    assert_eq!(post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":workspace["id"],"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
    let verified = body(
        post(
            &app,
            &owner,
            &format!(
                "/api/v1/workspaces/{}/runs",
                workspace["id"].as_str().unwrap()
            ),
            json!({"idempotency_key":Uuid::now_v7(),"expected_version":1}),
        )
        .await,
    )
    .await;
    let verified_id = verified["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    finish_run(&pool, verified_id, "42").await;
    assert!(
        sqlx::query("DELETE FROM workspace_runs WHERE id=$1")
            .bind(verified_id)
            .execute(&pool)
            .await
            .is_err()
    );
    let replay_id = body(
        post(
            &app,
            &owner,
            &format!(
                "/api/v1/workspaces/{}/runs",
                workspace["id"].as_str().unwrap()
            ),
            json!({"idempotency_key":Uuid::now_v7(),"expected_version":1}),
        )
        .await,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let replay_lease =
        alpha::workspaces::lease_next(&pool, "replay-worker", DECLARED_IMAGE, EXECUTION_IMAGE)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(replay_lease.id, replay_id);
    assert!(
        alpha::workspaces::mark_running(&pool, replay_id, replay_lease.lease_token)
            .await
            .unwrap()
    );
    let (copied_hash, copied_mac): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT runner_receipt_hash,runner_receipt_mac FROM workspace_runs WHERE id=$1",
    )
    .bind(verified_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE workspace_runs SET status='succeeded',lease_token=NULL,leased_by=NULL,lease_expires_at=NULL,execution_image_digest=$2,exit_code=0,stdout='42',stdout_hash=$3,deterministic_checks_passed=true,runner_receipt_hash=$4,runner_receipt_mac=$5,completed_at=now() WHERE id=$1")
        .bind(replay_id).bind(EXECUTION_IMAGE).bind(vec![1_u8;32]).bind(copied_hash).bind(copied_mac).execute(&pool).await.unwrap();
    assert_eq!(post(&app,&owner,"/api/v1/understanding/challenges",json!({"workspace_id":workspace["id"],"kind":"explanation_modification_transfer","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
    sqlx::query("INSERT INTO user_roles(user_id,role) VALUES($1,'RIGHTS_REVIEWER')")
        .bind(owner.user_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(post(&app,&owner,"/api/v1/portfolio/rights",json!({"source_run_id":verified_id,"provenance":"original","license_identifier":"Copyright-owner","idempotency_key":Uuid::now_v7()})).await.status(),StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn revisions_checkpoints_reset_and_expiry_are_owned_and_fenced(pool: PgPool) {
    let app = app(pool.clone());
    let owner = session(&app, "revision-owner").await;
    let other = session(&app, "revision-other").await;
    let make = |title: &str| json!({"title":title,"template_slug":"python-cli-v1","project_id":null,"files":files("41"),"idempotency_key":Uuid::now_v7()});
    let first = body(post(&app, &owner, "/api/v1/workspaces", make("첫 작업공간")).await).await;
    let second = body(post(&app, &owner, "/api/v1/workspaces", make("둘 작업공간")).await).await;
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    let key = Uuid::now_v7();
    let save = json!({"files":files("42"),"expected_version":1,"idempotency_key":key});
    assert_eq!(
        put(
            &app,
            &owner,
            &format!("/api/v1/workspaces/{first_id}"),
            save.clone()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        put(
            &app,
            &owner,
            &format!("/api/v1/workspaces/{second_id}"),
            save
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let checkpoint_key = Uuid::now_v7();
    let uri = format!("/api/v1/workspaces/{first_id}/checkpoints");
    let checkpoint_payload = json!({"name":"안정 버전","idempotency_key":checkpoint_key});
    let (a, b) = tokio::join!(
        post(&app, &owner, &uri, checkpoint_payload.clone()),
        post(&app, &owner, &uri, checkpoint_payload)
    );
    assert!(matches!(a.status(), StatusCode::CREATED | StatusCode::OK));
    assert!(matches!(b.status(), StatusCode::CREATED | StatusCode::OK));
    assert_eq!(body(a).await["id"], body(b).await["id"]);
    assert_eq!(
        post(
            &app,
            &owner,
            &uri,
            json!({"name":"다른 이름","idempotency_key":checkpoint_key})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let revision_uri = format!("/api/v1/workspaces/{first_id}/revisions/2");
    let revision = get(&app, &owner, &revision_uri).await;
    assert_eq!(revision.status(), StatusCode::OK);
    assert!(
        sqlx::query(
            "UPDATE workspace_revisions SET artifact='[]' WHERE workspace_id=$1 AND version=2"
        )
        .bind(first_id.parse::<Uuid>().unwrap())
        .execute(&pool)
        .await
        .is_err()
    );
    assert!(
        !body(revision).await["changed_paths"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        get(&app, &other, &revision_uri).await.status(),
        StatusCode::NOT_FOUND
    );
    let reset_key = Uuid::now_v7();
    let reset_uri = format!("/api/v1/workspaces/{first_id}/reset");
    let reset = json!({"version":1,"expected_version":2,"idempotency_key":reset_key});
    let restored = post(&app, &owner, &reset_uri, reset.clone()).await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(body(restored).await["version"], 3);
    let replay = post(&app, &owner, &reset_uri, reset).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await["version"], 3);
    assert_eq!(
        post(
            &app,
            &owner,
            &reset_uri,
            json!({"version":2,"expected_version":2,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let run = body(
        post(
            &app,
            &owner,
            &format!("/api/v1/workspaces/{first_id}/runs"),
            json!({"expected_version":3,"idempotency_key":Uuid::now_v7()}),
        )
        .await,
    )
    .await;
    let run_id = run["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let lease =
        alpha::workspaces::lease_next(&pool, "expiry-worker", DECLARED_IMAGE, EXECUTION_IMAGE)
            .await
            .unwrap()
            .unwrap();
    assert!(
        alpha::workspaces::mark_running(&pool, run_id, lease.lease_token)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE project_workspaces SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(first_id.parse::<Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(alpha::workspaces::expire(&pool).await.unwrap(), 1);
    assert_eq!(
        put(
            &app,
            &owner,
            &format!("/api/v1/workspaces/{first_id}"),
            json!({"files":files("99"),"expected_version":3,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &owner,
            &format!("/api/v1/workspaces/{first_id}/runs"),
            json!({"expected_version":3,"idempotency_key":Uuid::now_v7()})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let outcome = alpha::sandbox::WorkspaceOutcome {
        status: "succeeded",
        exit_code: Some(0),
        stdout: "late".into(),
        stderr: String::new(),
        output_truncated: false,
    };
    assert!(
        !alpha::workspaces::complete_run(
            &pool,
            run_id,
            lease.lease_token,
            &outcome,
            b"alpha-local-workspace-receipt-secret-32"
        )
        .await
        .unwrap()
    );
    alpha::workspaces::finish_cancel(&pool, run_id, lease.lease_token)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM workspace_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
}

#[sqlx::test(migrations = "./migrations")]
async fn signed_static_preview_and_instructor_revision_are_scoped(pool: PgPool) {
    let app = app(pool.clone());
    let instructor = session(&app, "snap-instructor").await;
    let learner = session(&app, "snap-learner").await;
    let workspace=body(post(&app,&learner,"/api/v1/workspaces",json!({"title":"정적 미리보기","template_slug":"static-web-v1","project_id":null,"files":[{"path":"index.html","content":"<!doctype html><html><body><button>안녕</button></body></html>","symlink":false}],"idempotency_key":Uuid::now_v7()})).await).await;
    assert_eq!(workspace["dependency_cache_status"], "not_required");
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let run = body(
        post(
            &app,
            &learner,
            &format!("/api/v1/workspaces/{workspace_id}/runs"),
            json!({"expected_version":1,"idempotency_key":Uuid::now_v7()}),
        )
        .await,
    )
    .await;
    let run_id = run["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    finish_run(&pool, run_id, "structure ok").await;
    let detail = body(get(&app, &learner, &format!("/api/v1/workspace-runs/{run_id}")).await).await;
    assert!(
        detail["preview_html"]
            .as_str()
            .unwrap()
            .contains("<button>안녕</button>")
    );

    let organization = Uuid::now_v7();
    let class = Uuid::now_v7();
    let other_class = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations(id,slug,name,created_by) VALUES($1,$2,'검증 기관',$3)")
        .bind(organization)
        .bind(format!("snapshot-{organization}"))
        .bind(instructor.user_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO classes(id,organization_id,name,created_by) VALUES($1,$2,'검증 반',$3),($4,$2,'다른 반',$3)").bind(class).bind(organization).bind(instructor.user_id).bind(other_class).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO class_memberships(class_id,user_id,role) VALUES($1,$2,'INSTRUCTOR'),($1,$3,'LEARNER')").bind(class).bind(instructor.user_id).bind(learner.user_id).execute(&pool).await.unwrap();
    let uri = format!(
        "/api/v1/classes/{class}/learners/{}/workspaces/{workspace_id}/revisions/1",
        learner.user_id
    );
    let snapshot = get(&app, &instructor, &uri).await;
    assert_eq!(snapshot.status(), StatusCode::OK);
    assert_eq!(
        body(snapshot).await["workspace_id"],
        workspace_id.to_string()
    );
    assert_eq!(
        get(&app, &learner, &uri).await.status(),
        StatusCode::FORBIDDEN
    );
    let cross = format!(
        "/api/v1/classes/{class}/learners/{}/workspaces/{workspace_id}/revisions/1",
        instructor.user_id
    );
    assert_eq!(
        get(&app, &instructor, &cross).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM audit_events WHERE actor_user_id=$1 AND action='workspace.revision.viewed_by_instructor'").bind(instructor.user_id).fetch_one(&pool).await.unwrap(),1);
}
