use std::{env, time::Duration};

use alpha::{
    db,
    judge::{self, Verdict},
    sandbox::{DockerSandbox, JudgeOutcome},
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn immutable_image_reference(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(_, digest)| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

async fn heartbeat(
    pool: &sqlx::PgPool,
    worker_id: &str,
    image: &str,
    status: &str,
    current_job_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO judge_workers (
            worker_id, protocol_version, image_reference, status, current_job_id
        ) VALUES ($1, 1, $2, $3, $4)
        ON CONFLICT (worker_id) DO UPDATE
        SET protocol_version = 1, image_reference = EXCLUDED.image_reference,
            status = EXCLUDED.status, current_job_id = EXCLUDED.current_job_id,
            last_heartbeat_at = now()
        "#,
    )
    .bind(worker_id)
    .bind(image)
    .bind(status)
    .bind(current_job_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "alpha=info".into()))
        .init();
    let database_url = env::var("DATABASE_URL")?;
    let app_env = env::var("APP_ENV").unwrap_or_else(|_| "development".to_owned());
    let image = env::var("JUDGE_IMAGE").unwrap_or_else(|_| "alpha-judge-runner:local".to_owned());
    if app_env == "production" && !immutable_image_reference(&image) {
        return Err("production JUDGE_IMAGE는 sha256 digest로 고정해야 합니다".into());
    }
    let worker_id = env::var("JUDGE_WORKER_ID")
        .unwrap_or_else(|_| format!("judge-worker-{}", std::process::id()));
    if !(3..=120).contains(&worker_id.len()) {
        return Err("JUDGE_WORKER_ID 길이는 3~120자여야 합니다".into());
    }
    let docker_binary = env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_owned());
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    let sandbox = DockerSandbox::new(docker_binary, image)?;
    sandbox.verify().await?;
    heartbeat(&pool, &worker_id, sandbox.image_reference(), "ready", None).await?;
    info!(worker_id = %worker_id, protocol_version = 1, "판정 작업자 시작");

    let mut shutdown = tokio::spawn(alpha::shutdown::signal());
    loop {
        if let Some(job) = judge::lease_next_job(&pool, &worker_id, judge::MAX_JOB_LEASE).await? {
            heartbeat(
                &pool,
                &worker_id,
                sandbox.image_reference(),
                "busy",
                Some(job.job_id),
            )
            .await?;
            info!(job_id = %job.job_id, submission_id = %job.submission_id, attempt = job.attempt, "판정 작업 시작");
            let test_cases =
                judge::load_test_cases(&pool, job.problem_revision_id, &job.run_kind).await;
            let outcome = match test_cases {
                Ok(test_cases) => {
                    if let Err(error) =
                        judge::record_running(&pool, job.job_id, job.lease_token).await
                    {
                        warn!(job_id = %job.job_id, %error, "RUNNING 상태 기록 실패");
                    }
                    match sandbox.judge(&job, &test_cases).await {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            error!(job_id = %job.job_id, %error, "샌드박스 판정 실패");
                            JudgeOutcome {
                                verdict: Verdict::SystemError,
                                score: 0,
                                compile_output: None,
                                run_output: None,
                            }
                        }
                    }
                }
                Err(error) => {
                    error!(job_id = %job.job_id, %error, "판정 테스트 조회 실패");
                    JudgeOutcome {
                        verdict: Verdict::SystemError,
                        score: 0,
                        compile_output: None,
                        run_output: None,
                    }
                }
            };
            match judge::complete_job_with_output(
                &pool,
                job.job_id,
                job.lease_token,
                outcome.verdict,
                outcome.score,
                outcome.compile_output,
                outcome.run_output,
            )
            .await
            {
                Ok(()) => {
                    info!(job_id = %job.job_id, verdict = outcome.verdict.as_str(), score = outcome.score, "판정 작업 완료")
                }
                Err(error) => warn!(job_id = %job.job_id, %error, "판정 결과 저장 실패"),
            }
            heartbeat(&pool, &worker_id, sandbox.image_reference(), "ready", None).await?;
            continue;
        }

        heartbeat(&pool, &worker_id, sandbox.image_reference(), "ready", None).await?;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = &mut shutdown => {
                heartbeat(&pool, &worker_id, sandbox.image_reference(), "draining", None).await?;
                info!(worker_id = %worker_id, "판정 작업자 종료");
                break;
            }
        }
    }
    Ok(())
}
