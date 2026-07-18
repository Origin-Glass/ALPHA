use std::{env, time::Duration};

use alpha::{db, metadata};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

async fn refresh_due(pool: &sqlx::PgPool, client: &reqwest::Client) -> Result<usize, sqlx::Error> {
    let external_ids: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT external.external_problem_id
        FROM problem_external_links external
        LEFT JOIN external_problem_metadata_cache metadata
          ON metadata.provider = external.provider
         AND metadata.external_problem_id = external.external_problem_id
        WHERE external.provider = 'solved_ac'
          AND (metadata.provider IS NULL OR (metadata.expires_at <= now() AND metadata.state <> 'blocked'))
        ORDER BY external.external_problem_id
        LIMIT 100
        "#,
    )
    .fetch_all(pool)
    .await?;

    for external_id in &external_ids {
        if let Err(error) = metadata::refresh_solved_ac_metadata(pool, client, external_id).await {
            warn!(%error, external_problem_id = %external_id, "solved.ac 메타데이터 갱신 실패");
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    Ok(external_ids.len())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "alpha=info".into()))
        .init();
    let database_url = env::var("DATABASE_URL")?;
    let interval_seconds = env::var("METADATA_REFRESH_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3600)
        .clamp(60, 86_400);
    let enabled = env::var("SOLVED_AC_ENABLED").as_deref() == Ok("true");
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .user_agent("Origin-Glass-ALPHA/1.0 metadata-worker")
        .build()?;
    let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));

    if !enabled {
        info!("solved.ac 메타데이터 작업자는 비활성화 상태입니다");
        alpha::shutdown::signal().await;
        return Ok(());
    }

    let mut shutdown = tokio::spawn(alpha::shutdown::signal());
    loop {
        tokio::select! {
            _ = interval.tick() => {
                match refresh_due(&pool, &client).await {
                    Ok(count) => info!(count, "외부 문제 메타데이터 주기 갱신 완료"),
                    Err(error) => warn!(%error, "외부 문제 메타데이터 대상 조회 실패"),
                }
            }
            _ = &mut shutdown => {
                info!("메타데이터 작업자 종료");
                break;
            }
        }
    }
    Ok(())
}
