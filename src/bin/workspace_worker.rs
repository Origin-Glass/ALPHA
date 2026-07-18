use alpha::{db, sandbox::DockerSandbox, workspaces};
use std::{env, time::Duration};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn immutable_image_reference(image: &str) -> bool {
    image
        .rsplit_once("@sha256:")
        .is_some_and(|(_, d)| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "alpha=info".into()))
        .init();
    let database_url = env::var("DATABASE_URL")?;
    let image = env::var("WORKSPACE_IMAGE")?;
    let app_env = env::var("APP_ENV").unwrap_or_else(|_| "development".into());
    if app_env == "production" && !immutable_image_reference(&image) {
        return Err("WORKSPACE_IMAGE는 sha256 digest로 고정해야 합니다".into());
    }
    let declared_image_reference = if app_env == "production" {
        image.clone()
    } else {
        env::var("WORKSPACE_DECLARED_IMAGE_REFERENCE")?
    };
    if !immutable_image_reference(&declared_image_reference) {
        return Err("WORKSPACE_DECLARED_IMAGE_REFERENCE는 sha256 digest로 고정해야 합니다".into());
    }
    let receipt_secret = env::var("WORKSPACE_RECEIPT_SECRET")?;
    if receipt_secret.len() < 32 {
        return Err("WORKSPACE_RECEIPT_SECRET은 32바이트 이상이어야 합니다".into());
    }
    let worker = env::var("WORKSPACE_WORKER_ID")
        .unwrap_or_else(|_| format!("workspace-worker-{}", std::process::id()));
    let docker = env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".into());
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    let sandbox = DockerSandbox::new(docker, image.clone())?;
    sandbox.verify().await?;
    let execution_image_digest = sandbox.immutable_image_id().await?;
    let mut shutdown = tokio::spawn(alpha::shutdown::signal());
    info!(%worker,%image,"작업공간 작업자 시작");
    loop {
        let _ = workspaces::expire(&pool).await?;
        if let Some(job) = workspaces::lease_next(
            &pool,
            &worker,
            &declared_image_reference,
            &execution_image_digest,
        )
        .await?
        {
            if !workspaces::mark_running(&pool, job.id, job.lease_token).await? {
                workspaces::finish_cancel(&pool, job.id, job.lease_token).await?;
                continue;
            }
            let run = sandbox.run_workspace(job.id, &job.files, &job.command);
            tokio::pin!(run);
            let outcome = loop {
                tokio::select! {result=&mut run=>break Some(result),_=tokio::time::sleep(Duration::from_millis(200))=>{if workspaces::run_cancelled(&pool,job.id,job.lease_token).await?{break None}}}
            };
            match outcome {
                Some(Ok(out)) => {
                    if !workspaces::complete_or_finish_cancel(
                        &pool,
                        job.id,
                        job.lease_token,
                        &out,
                        receipt_secret.as_bytes(),
                    )
                    .await?
                    {
                        warn!(run_id=%job.id,"임대 만료 또는 취소로 결과 폐기")
                    }
                }
                Some(Err(e)) => {
                    warn!(run_id=%job.id,%e,"작업공간 실행 실패");
                    workspaces::finish_cancel(&pool, job.id, job.lease_token).await?
                }
                None => {
                    sandbox.cleanup_workspace(job.id).await;
                    workspaces::finish_cancel(&pool, job.id, job.lease_token).await?
                }
            }
            continue;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=&mut shutdown=>break}
    }
    Ok(())
}
