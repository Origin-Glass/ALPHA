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

fn provider_request(
    client: &reqwest::Client,
    base_url: &url::Url,
    lease: &alpha::content_factory::GenerationLease,
    credential: Option<&str>,
) -> Result<reqwest::RequestBuilder, &'static str> {
    let prompt = format!(
        "다음 명세에 맞는 한국어 교육 콘텐츠를 JSON으로 생성하세요: {}",
        lease.request_spec
    );
    match lease.provider_protocol.as_str() {
        "openai_compatible" => {
            let url = base_url
                .join("v1/chat/completions")
                .map_err(|_| "provider_url_join_error")?;
            let request = client.post(url).json(&json!({
                "model": lease.model,
                "messages": [{"role": "user", "content": prompt}],
                "response_format": {"type": "json_object"}
            }));
            Ok(match credential {
                Some(credential) => request.bearer_auth(credential),
                None => request,
            })
        }
        "anthropic" => {
            let credential = credential.ok_or("credential_missing_at_execution")?;
            let url = base_url
                .join("v1/messages")
                .map_err(|_| "provider_url_join_error")?;
            Ok(client
                .post(url)
                .header("x-api-key", credential)
                .header("anthropic-version", "2023-06-01")
                .json(&json!({
                    "model": lease.model,
                    "max_tokens": 4096,
                    "messages": [{"role": "user", "content": prompt}]
                })))
        }
        _ => Err("provider_protocol_rejected"),
    }
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
        Err(_) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "provider_client_configuration_error",
                json!({"error": "제공자 클라이언트 구성 실패"}),
                false,
            )
            .await;
            return;
        }
    };
    let request = match provider_request(&client, &endpoint.url, &lease, credential.as_deref()) {
        Ok(request) => request,
        Err(error_code) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                error_code,
                json!({"error": "제공자 프로토콜 구성 실패"}),
                false,
            )
            .await;
            return;
        }
    };
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "provider_transport_error",
                json!({"error": "제공자 전송 실패"}),
                true,
            )
            .await;
            return;
        }
    };
    let status = response.status();
    if !status.is_success() {
        let _ = fail_generation_job(
            pool,
            lease.job_id,
            lease.attempt_id,
            lease.lease_token,
            "provider_http_error",
            json!({"error": "제공자 HTTP 오류", "status": status.as_u16()}),
            retryable_status(status),
        )
        .await;
        return;
    }
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        let _ = fail_generation_job(
            pool,
            lease.job_id,
            lease.attempt_id,
            lease.lease_token,
            "provider_response_too_large",
            json!({"error": "제공자 응답 크기 초과"}),
            false,
        )
        .await;
        return;
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_RESPONSE_BYTES => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => {
                let _ = fail_generation_job(
                    pool,
                    lease.job_id,
                    lease.attempt_id,
                    lease.lease_token,
                    "provider_response_too_large",
                    json!({"error": "제공자 응답 크기 초과"}),
                    false,
                )
                .await;
                return;
            }
            Ok(None) => break,
            Err(_) => {
                let _ = fail_generation_job(
                    pool,
                    lease.job_id,
                    lease.attempt_id,
                    lease.lease_token,
                    "provider_response_read_error",
                    json!({"error": "제공자 응답 읽기 실패"}),
                    true,
                )
                .await;
                return;
            }
        }
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "provider_response_invalid_json",
                json!({"error": "제공자 응답 형식 오류"}),
                false,
            )
            .await;
            return;
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn lease(protocol: &str) -> alpha::content_factory::GenerationLease {
        alpha::content_factory::GenerationLease {
            job_id: Uuid::nil(),
            attempt_id: Uuid::nil(),
            attempt_number: 1,
            lease_token: Uuid::nil(),
            provider_kind: "external".into(),
            provider_protocol: protocol.into(),
            base_url: "https://api.example.com".into(),
            model: "contract-model".into(),
            credential_env_var: Some(if protocol == "anthropic" {
                "ANTHROPIC_API_KEY".into()
            } else {
                "OPENAI_API_KEY".into()
            }),
            request_spec: json!({"topic": "경계 조건"}),
            reserved_cost_microunits: 100,
        }
    }

    fn body(request: &reqwest::Request) -> Value {
        serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap()
    }

    #[test]
    fn anthropic_adapter_uses_messages_contract_and_anthropic_headers() {
        let client = reqwest::Client::new();
        let request = provider_request(
            &client,
            &url::Url::parse("https://api.anthropic.com").unwrap(),
            &lease("anthropic"),
            Some("test-anthropic-key"),
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.url().path(), "/v1/messages");
        assert_eq!(request.headers()["x-api-key"], "test-anthropic-key");
        assert_eq!(request.headers()["anthropic-version"], "2023-06-01");
        assert_eq!(body(&request)["max_tokens"], 4096);
        assert!(body(&request).get("response_format").is_none());
    }

    #[test]
    fn openai_adapter_uses_chat_completions_and_bearer_auth() {
        let client = reqwest::Client::new();
        let request = provider_request(
            &client,
            &url::Url::parse("https://api.openai.com").unwrap(),
            &lease("openai_compatible"),
            Some("test-openai-key"),
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.url().path(), "/v1/chat/completions");
        assert_eq!(request.headers()["authorization"], "Bearer test-openai-key");
        assert_eq!(body(&request)["response_format"]["type"], "json_object");
        assert!(body(&request).get("max_tokens").is_none());
    }
}
