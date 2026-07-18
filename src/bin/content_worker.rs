use std::time::Duration;

use alpha::content_factory::{
    CONTENT_PROMPT_TEMPLATE, CompleteGenerationError, begin_provider_call, complete_generation_job,
    fail_generation_job, lease_next_generation_job, record_provider_output, renew_generation_lease,
    resolve_provider_endpoint,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tracing_subscriber::EnvFilter;

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn provider_request(
    client: &reqwest::Client,
    base_url: &url::Url,
    lease: &alpha::content_factory::GenerationLease,
    credential: Option<&str>,
    output_index: u64,
) -> Result<reqwest::RequestBuilder, &'static str> {
    let prompt = format!("{CONTENT_PROMPT_TEMPLATE}{}", lease.request_spec);
    let idempotency_key = format!("alpha-content-{}-{output_index}", lease.job_id);
    let seed = deterministic_seed(lease.job_id, output_index);
    match lease.provider_protocol.as_str() {
        "openai_compatible" => {
            let url = base_url
                .join("v1/chat/completions")
                .map_err(|_| "provider_url_join_error")?;
            let request = client
                .post(url)
                .header("idempotency-key", &idempotency_key)
                .json(&json!({
                    "model": lease.model,
                    "seed": seed,
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
                .header("idempotency-key", &idempotency_key)
                .header("x-api-key", credential)
                .header("anthropic-version", "2023-06-01")
                .json(&json!({
                    "model": lease.model,
                    "max_tokens": 4096,
                    "metadata": {"user_id": idempotency_key},
                    "messages": [{"role": "user", "content": prompt}]
                })))
        }
        _ => Err("provider_protocol_rejected"),
    }
}

fn deterministic_seed(job_id: uuid::Uuid, output_index: u64) -> u64 {
    let digest = Sha256::digest(format!("{job_id}:{output_index}").as_bytes());
    u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 prefix is eight bytes"),
    )
}

fn generation_count(request_spec: &Value) -> Result<u64, &'static str> {
    request_spec["generation_count"]
        .as_u64()
        .filter(|count| (1..=20).contains(count))
        .ok_or("invalid_generation_count_snapshot")
}

fn normalized_content(content_type: &str, text: &str) -> Result<Value, &'static str> {
    let content: Value = serde_json::from_str(text).map_err(|_| "provider_content_invalid_json")?;
    let object = content
        .as_object()
        .ok_or("provider_content_schema_invalid")?;
    let required: &[&str] = match content_type {
        "algorithm_problem" => &["title", "statement", "solution"],
        "code_reading" => &["title", "code", "question", "answer"],
        "debugging" => &["title", "buggy_code", "explanation", "fixed_code"],
        "documentation_lesson" => &["title", "lesson"],
        "implementation_task" => &["title", "requirements", "reference_solution"],
        _ => return Err("provider_content_schema_invalid"),
    };
    if required.iter().any(|field| {
        object.get(*field).is_none_or(|value| match value {
            Value::String(value) => value.trim().is_empty(),
            Value::Array(value) => value.is_empty(),
            Value::Null => true,
            _ => false,
        })
    }) {
        return Err("provider_content_schema_invalid");
    }
    Ok(content)
}

fn normalize_provider_output(
    protocol: &str,
    content_type: &str,
    payload: &Value,
    latency_ms: u64,
) -> Result<Value, &'static str> {
    let (text, usage) = match protocol {
        "openai_compatible" => {
            let text = payload
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .ok_or("provider_content_missing")?;
            let usage = json!({
                "prompt_tokens": payload.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
                "completion_tokens": payload.pointer("/usage/completion_tokens").and_then(Value::as_u64),
                "total_tokens": payload.pointer("/usage/total_tokens").and_then(Value::as_u64),
            });
            (text.to_owned(), usage)
        }
        "anthropic" => {
            let text = payload["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                return Err("provider_content_missing");
            }
            let usage = json!({
                "input_tokens": payload.pointer("/usage/input_tokens").and_then(Value::as_u64),
                "output_tokens": payload.pointer("/usage/output_tokens").and_then(Value::as_u64),
            });
            (text, usage)
        }
        _ => return Err("provider_protocol_rejected"),
    };
    Ok(json!({
        "content": normalized_content(content_type, &text)?,
        "metadata": {"usage": usage, "latency_ms": latency_ms}
    }))
}

async fn record_provider_health(pool: &sqlx::PgPool, provider_id: uuid::Uuid, status: &str) {
    if let Err(error) = sqlx::query(
        r#"UPDATE content_provider_configs
           SET health_status = $2, last_health_at = now(),
               consecutive_failures = CASE WHEN $2 = 'healthy' THEN 0 ELSE consecutive_failures + 1 END,
               updated_at = now() WHERE id = $1"#,
    )
    .bind(provider_id)
    .bind(status)
    .execute(pool)
    .await
    {
        tracing::warn!(%provider_id, %error, "제공자 상태 기록 실패");
    }
}

struct ProviderFailure {
    code: &'static str,
    receipt: Value,
    retryable: bool,
}

async fn call_provider(
    client: &reqwest::Client,
    endpoint: &url::Url,
    lease: &alpha::content_factory::GenerationLease,
    credential: Option<&str>,
    output_index: u64,
) -> Result<Value, ProviderFailure> {
    let request =
        provider_request(client, endpoint, lease, credential, output_index).map_err(|code| {
            ProviderFailure {
                code,
                receipt: json!({"error": "제공자 프로토콜 구성 실패"}),
                retryable: false,
            }
        })?;
    let started = std::time::Instant::now();
    let mut response = request.send().await.map_err(|_| ProviderFailure {
        code: "provider_transport_error",
        receipt: json!({"error": "제공자 전송 실패"}),
        retryable: true,
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProviderFailure {
            code: "provider_http_error",
            receipt: json!({"error": "제공자 HTTP 오류", "status": status.as_u16()}),
            retryable: retryable_status(status),
        });
    }
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProviderFailure {
            code: "provider_response_too_large",
            receipt: json!({"error": "제공자 응답 크기 초과"}),
            retryable: false,
        });
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_RESPONSE_BYTES => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => {
                return Err(ProviderFailure {
                    code: "provider_response_too_large",
                    receipt: json!({"error": "제공자 응답 크기 초과"}),
                    retryable: false,
                });
            }
            Ok(None) => break,
            Err(_) => {
                return Err(ProviderFailure {
                    code: "provider_response_read_error",
                    receipt: json!({"error": "제공자 응답 읽기 실패"}),
                    retryable: true,
                });
            }
        }
    }
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ProviderFailure {
        code: "provider_response_invalid_json",
        receipt: json!({"error": "제공자 응답 형식 오류"}),
        retryable: false,
    })?;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    normalize_provider_output(
        &lease.provider_protocol,
        &lease.content_type,
        &payload,
        latency_ms,
    )
    .map_err(|code| ProviderFailure {
        code,
        receipt: json!({"error": "제공자 콘텐츠가 비어 있음"}),
        retryable: false,
    })
}

async fn call_with_lease_heartbeat(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    endpoint: &url::Url,
    lease: &alpha::content_factory::GenerationLease,
    credential: Option<&str>,
    output_index: u64,
) -> Option<Result<Value, ProviderFailure>> {
    let call = call_provider(client, endpoint, lease, credential, output_index);
    tokio::pin!(call);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut call => return Some(result),
            _ = heartbeat.tick() => {
                match renew_generation_lease(pool, lease.job_id, lease.attempt_id, lease.lease_token, Duration::from_secs(120)).await {
                    Ok(true) => {}
                    Ok(false) | Err(_) => return None,
                }
            }
        }
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
    let generation_count = match generation_count(&lease.request_spec) {
        Ok(count) => count,
        Err(code) => {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                code,
                json!({"error": "생성 수 스냅샷이 유효하지 않음"}),
                false,
            )
            .await;
            return;
        }
    };
    let mut candidates = Vec::with_capacity(generation_count as usize);
    let batch_deadline = tokio::time::Instant::now() + Duration::from_secs(30 * 60);
    for output_index in 0..generation_count {
        if tokio::time::Instant::now() >= batch_deadline {
            let _ = fail_generation_job(
                pool,
                lease.job_id,
                lease.attempt_id,
                lease.lease_token,
                "batch_deadline_exceeded",
                json!({"error": "배치 실행 제한 시간 초과"}),
                true,
            )
            .await;
            return;
        }
        if !matches!(
            begin_provider_call(
                pool,
                lease.job_id,
                lease.provider_id,
                lease.attempt_id,
                lease.lease_token
            )
            .await,
            Ok(true)
        ) {
            return;
        }
        let Some(result) = call_with_lease_heartbeat(
            pool,
            &client,
            &endpoint.url,
            &lease,
            credential.as_deref(),
            output_index,
        )
        .await
        else {
            return;
        };
        match result {
            Ok(mut output) => {
                output["metadata"]["output_index"] = json!(output_index);
                output["metadata"]["seed"] = json!(deterministic_seed(lease.job_id, output_index));
                let usage = json!({output_index.to_string(): output["metadata"]["usage"].clone()});
                if !matches!(
                    record_provider_output(
                        pool,
                        lease.job_id,
                        lease.attempt_id,
                        lease.lease_token,
                        &usage
                    )
                    .await,
                    Ok(true)
                ) {
                    return;
                }
                candidates.push(output);
            }
            Err(failure) => {
                record_provider_health(pool, lease.provider_id, "unhealthy").await;
                let _ = fail_generation_job(
                    pool,
                    lease.job_id,
                    lease.attempt_id,
                    lease.lease_token,
                    failure.code,
                    failure.receipt,
                    failure.retryable,
                )
                .await;
                return;
            }
        }
    }
    let payload = json!({
        "candidates": candidates,
        "generation_count": generation_count,
        "metadata": {
            "prompt_template_version": "content-v1",
            "seed_policy": "sha256(job_id:output_index)",
            "provider_idempotency_key_policy": "job_id:output_index"
        }
    });
    record_provider_health(pool, lease.provider_id, "healthy").await;
    if let Err(error) = complete_generation_job(
        pool,
        lease.job_id,
        lease.attempt_id,
        lease.lease_token,
        payload,
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
            provider_id: Uuid::nil(),
            provider_protocol: protocol.into(),
            base_url: "https://api.example.com".into(),
            model: "contract-model".into(),
            credential_env_var: Some(if protocol == "anthropic" {
                "ANTHROPIC_API_KEY".into()
            } else {
                "OPENAI_API_KEY".into()
            }),
            request_spec: json!({"topic": "경계 조건", "generation_count": 3}),
            reserved_cost_microunits: 300,
            attempt_cost_microunits: 100,
            content_type: "code_reading".into(),
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
            0,
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.url().path(), "/v1/messages");
        assert_eq!(request.headers()["x-api-key"], "test-anthropic-key");
        assert_eq!(request.headers()["anthropic-version"], "2023-06-01");
        assert_eq!(
            request.headers()["idempotency-key"],
            "alpha-content-00000000-0000-0000-0000-000000000000-0"
        );
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
            0,
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.url().path(), "/v1/chat/completions");
        assert_eq!(request.headers()["authorization"], "Bearer test-openai-key");
        assert_eq!(body(&request)["response_format"]["type"], "json_object");
        assert!(body(&request).get("max_tokens").is_none());
    }

    #[test]
    fn generation_count_produces_distinct_auditable_provider_requests() {
        let client = reqwest::Client::new();
        let lease = lease("openai_compatible");
        let count = lease.request_spec["generation_count"].as_u64().unwrap();
        let requests = (0..count)
            .map(|index| {
                provider_request(
                    &client,
                    &url::Url::parse("https://api.openai.com").unwrap(),
                    &lease,
                    Some("test-key"),
                    index,
                )
                .unwrap()
                .build()
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 3);
        let keys = requests
            .iter()
            .map(|request| request.headers()["idempotency-key"].to_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(keys.len(), 3);
        let seeds = requests
            .iter()
            .map(|request| body(request)["seed"].as_u64().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(seeds.len(), 3);
    }

    #[test]
    fn malformed_generation_count_snapshot_is_rejected_instead_of_under_generating() {
        assert_eq!(generation_count(&json!({"generation_count": 3})), Ok(3));
        for malformed in [
            json!({}),
            json!({"generation_count": 0}),
            json!({"generation_count": 21}),
            json!({"generation_count": "3"}),
        ] {
            assert_eq!(
                generation_count(&malformed),
                Err("invalid_generation_count_snapshot")
            );
        }
    }

    #[test]
    fn normalizer_keeps_only_openai_content_and_safe_usage() {
        let normalized = normalize_provider_output(
            "openai_compatible",
            "code_reading",
            &json!({
                "choices": [{"message": {"content": "{\"title\":\"경계 조건\",\"code\":\"x\",\"question\":\"왜?\",\"answer\":\"불변식\"}", "reasoning": "hidden"}}],
                "usage": {"prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19, "secret": "drop"},
                "internal_trace": "drop-me"
            }),
            41,
        )
        .unwrap();
        assert_eq!(normalized["content"]["title"], "경계 조건");
        assert_eq!(normalized["metadata"]["usage"]["total_tokens"], 19);
        assert_eq!(normalized["metadata"]["latency_ms"], 41);
        let encoded = normalized.to_string();
        assert!(!encoded.contains("hidden"));
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("internal_trace"));
    }

    #[test]
    fn normalizer_extracts_anthropic_text_without_raw_blocks() {
        let normalized = normalize_provider_output(
            "anthropic",
            "documentation_lesson",
            &json!({
                "content": [
                    {"type": "thinking", "thinking": "private chain"},
                    {"type": "text", "text": "{\"title\":\"불변식\",\"lesson\":\"불변식\"}"}
                ],
                "usage": {"input_tokens": 5, "output_tokens": 9},
                "debug": "drop-me"
            }),
            55,
        )
        .unwrap();
        assert_eq!(normalized["content"]["lesson"], "불변식");
        assert_eq!(normalized["metadata"]["usage"]["input_tokens"], 5);
        let encoded = normalized.to_string();
        assert!(!encoded.contains("private chain"));
        assert!(!encoded.contains("debug"));
    }

    #[test]
    fn normalizer_rejects_non_json_and_incomplete_content() {
        for content in [
            "설명부터 시작합니다 {\"title\":\"숨김\"}",
            "{\"title\":\"누락\"}",
        ] {
            assert!(matches!(
                normalize_provider_output(
                    "openai_compatible",
                    "debugging",
                    &json!({"choices": [{"message": {"content": content}}]}),
                    1,
                ),
                Err("provider_content_invalid_json" | "provider_content_schema_invalid")
            ));
        }
    }
}
