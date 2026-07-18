use std::time::Duration;

use alpha::content_factory::{
    CompleteGenerationError, complete_generation_job, fail_generation_job,
    lease_next_generation_job, resolve_provider_endpoint,
};
use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

async fn run_lease(pool: &sqlx::PgPool, lease: alpha::content_factory::GenerationLease) {
    let endpoint = match resolve_provider_endpoint(&lease.provider_kind, &lease.base_url).await {
        Ok(endpoint) => endpoint,
        Err(_) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "provider_endpoint_rejected",
                json!({"error": "호출 직전 제공자 주소 검증 실패"}),
                false,
            )
            .await;
            return;
        }
    };
    let credential = match lease.credential_env_var.as_deref() {
        Some(name) => match std::env::var(name).ok().filter(|value| !value.is_empty()) {
            Some(value) => Some(value),
            None => {
                let _ = fail_generation_job(
                    pool,
                    lease.job_id,
                    lease.attempt_id,
                    lease.lease_token,
                    "credential_missing_at_execution",
                    json!({"error": "실행 시점 자격 증명 없음"}),
                    false,
                )
                .await;
                return;
            }
        },
        None => None,
    };
    let mut client_builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90));
    if let Some((domain, address)) = endpoint.dns_pin {
        client_builder = client_builder.resolve(&domain, address);
    }
    let client = match client_builder.build() {
        Ok(client) => client,
        Err(_) => return,
    };
    let request_url = match endpoint.url.join("v1/chat/completions") {
        Ok(url) => url,
        Err(_) => return,
    };
    let mut request = client.post(request_url).json(&json!({
        "model": lease.model,
        "messages": [{
            "role": "user",
            "content": format!("다음 명세에 맞는 한국어 교육 콘텐츠를 JSON으로 생성하세요: {}", lease.request_spec)
        }],
        "response_format": {"type": "json_object"}
    }));
    if let Some(credential) = credential {
        request = request.bearer_auth(credential);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "provider_transport_error",
                json!({"error": error.to_string()}),
                true,
            )
            .await;
            return;
        }
    };
    let status = response.status();
    let payload: Value = match response.json().await {
        Ok(payload) => payload,
        Err(_) => json!({"error": "제공자 응답이 JSON이 아님", "status": status.as_u16()}),
    };
    if !status.is_success() {
        let _ = fail_generation_job(
            pool,
            lease.job_id,
            lease.attempt_id,
            lease.lease_token,
            "provider_http_error",
            json!({"status": status.as_u16(), "response": payload}),
            retryable_status(status),
        )
        .await;
        return;
    }
    if let Err(error) = complete_generation_job(
        pool,
        lease.job_id,
        lease.attempt_id,
        lease.lease_token,
        payload,
        lease.reserved_cost_microunits,
    )
    .await
        && !matches!(error, CompleteGenerationError::LeaseLost)
    {
        tracing::error!(job_id = %lease.job_id, %error, "콘텐츠 작업 완료 처리 실패");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = alpha::db::connect(&database_url).await?;
    sqlx::migrate!().run(&pool).await?;
    if std::env::var("CONTENT_AI_ENABLED").as_deref() != Ok("true") {
        tracing::warn!("CONTENT_AI_ENABLED가 true가 아니므로 콘텐츠 worker를 시작하지 않습니다");
        return Ok(());
    }
    let worker_id =
        std::env::var("CONTENT_WORKER_ID").unwrap_or_else(|_| "content-worker-1".into());
    loop {
        tokio::select! {
            _ = alpha::shutdown::signal() => break,
            leased = lease_next_generation_job(&pool, &worker_id, Duration::from_secs(120)) => {
                match leased? {
                    Some(lease) => run_lease(&pool, lease).await,
                    None => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
        }
    }
    Ok(())
}
