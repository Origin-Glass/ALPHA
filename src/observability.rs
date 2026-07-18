use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::http::AppState;

const WINDOW: Duration = Duration::from_secs(60);
const READ_LIMIT: u32 = 600;
const WRITE_LIMIT: u32 = 60;

#[derive(Default)]
pub struct RuntimeMetrics {
    requests: AtomicU64,
    server_errors: AtomicU64,
    rate_entries: Mutex<HashMap<String, RateEntry>>,
}

struct RateEntry {
    started_at: Instant,
    reads: u32,
    writes: u32,
}

impl RuntimeMetrics {
    fn allow(&self, key: String, write: bool) -> bool {
        let now = Instant::now();
        let mut entries = self
            .rate_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() > 10_000 {
            entries.retain(|_, entry| now.duration_since(entry.started_at) < WINDOW);
        }
        let entry = entries.entry(key).or_insert(RateEntry {
            started_at: now,
            reads: 0,
            writes: 0,
        });
        if now.duration_since(entry.started_at) >= WINDOW {
            *entry = RateEntry {
                started_at: now,
                reads: 0,
                writes: 0,
            };
        }
        let (count, limit) = if write {
            (&mut entry.writes, WRITE_LIMIT)
        } else {
            (&mut entry.reads, READ_LIMIT)
        };
        if *count >= limit {
            return false;
        }
        *count += 1;
        true
    }

    pub fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn server_error_count(&self) -> u64 {
        self.server_errors.load(Ordering::Relaxed)
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

fn rate_key(state: &AppState, request: &Request<Body>) -> String {
    if let Some(session) = cookie_value(request.headers(), "alpha_session") {
        return format!(
            "session:{}",
            URL_SAFE_NO_PAD.encode(Sha256::digest(session.as_bytes()))
        );
    }
    if state.settings().trust_proxy_headers
        && let Some(ip) = request
            .headers()
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<IpAddr>().ok())
    {
        return format!("ip:{ip}");
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            || "peer:unknown".to_owned(),
            |peer| format!("ip:{}", peer.0.ip()),
        )
}

pub async fn track_and_limit(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    state.runtime().requests.fetch_add(1, Ordering::Relaxed);
    let write = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if !state.runtime().allow(rate_key(&state, &request), write) {
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": {
                "code": "rate_limited",
                "message": "요청이 너무 많습니다. 잠시 후 다시 시도해 주세요"
            }})),
        )
            .into_response();
        response.headers_mut().insert(
            header::RETRY_AFTER,
            "60".parse().expect("고정 헤더는 유효하다"),
        );
        return response;
    }
    let response = next.run(request).await;
    if response.status().is_server_error() {
        state
            .runtime()
            .server_errors
            .fetch_add(1, Ordering::Relaxed);
    }
    response
}
