use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::http::AppState;
use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, State},
    http::{Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

const WINDOW: Duration = Duration::from_secs(60);
const READ_LIMIT: u32 = 600;
const WRITE_LIMIT: u32 = 60;
const MAX_RATE_ENTRIES: usize = 10_000;
const OVERFLOW_KEY: &str = "peer:overflow";
const SSE_LIMIT_PER_USER: u8 = 4;

#[derive(Default)]
pub struct RuntimeMetrics {
    requests: AtomicU64,
    server_errors: AtomicU64,
    rate_entries: Mutex<HashMap<String, RateEntry>>,
    sse_streams: Mutex<HashMap<Uuid, u8>>,
}

pub struct SsePermit {
    runtime: Arc<RuntimeMetrics>,
    user_id: Uuid,
}

impl Drop for SsePermit {
    fn drop(&mut self) {
        let mut streams = self
            .runtime
            .sse_streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = streams.get_mut(&self.user_id) {
            *count -= 1;
            if *count == 0 {
                streams.remove(&self.user_id);
            }
        }
    }
}

struct RateEntry {
    started_at: Instant,
    reads: u32,
    writes: u32,
}

impl RuntimeMetrics {
    fn allow(&self, key: String, write: bool) -> bool {
        self.allow_with_limits(key, write, READ_LIMIT, WRITE_LIMIT)
    }

    fn allow_with_limits(
        &self,
        key: String,
        write: bool,
        read_limit: u32,
        write_limit: u32,
    ) -> bool {
        let now = Instant::now();
        let mut entries = self
            .rate_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() >= MAX_RATE_ENTRIES {
            entries.retain(|_, entry| now.duration_since(entry.started_at) < WINDOW);
        }
        let key = if entries.len() >= MAX_RATE_ENTRIES - 1 && !entries.contains_key(&key) {
            OVERFLOW_KEY.to_owned()
        } else {
            key
        };
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
            (&mut entry.writes, write_limit)
        } else {
            (&mut entry.reads, read_limit)
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

    pub fn acquire_sse(self: &Arc<Self>, user_id: Uuid) -> Option<SsePermit> {
        let mut streams = self
            .sse_streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = streams.entry(user_id).or_default();
        if *count >= SSE_LIMIT_PER_USER {
            return None;
        }
        *count += 1;
        Some(SsePermit {
            runtime: Arc::clone(self),
            user_id,
        })
    }
}

fn rate_key(state: &AppState, request: &Request<Body>) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotating_untrusted_keys_cannot_grow_the_rate_map_past_its_cap() {
        let metrics = RuntimeMetrics::default();
        for index in 0..MAX_RATE_ENTRIES + 500 {
            metrics.allow(format!("attacker-{index}"), false);
        }
        assert!(metrics.rate_entries.lock().unwrap().len() <= MAX_RATE_ENTRIES);
    }
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
    let peer_key = rate_key(&state, &request);
    let edge_allowed = state.runtime().allow_with_limits(
        format!("edge:{peer_key}"),
        write,
        READ_LIMIT * 10,
        WRITE_LIMIT * 10,
    );
    let subject_key = match crate::auth::authenticated_user_id(&state, request.headers()).await {
        Ok(user_id) => format!("user:{user_id}"),
        Err(_) => peer_key,
    };
    if !edge_allowed || !state.runtime().allow(subject_key, write) {
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
