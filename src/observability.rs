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
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

const WINDOW: Duration = Duration::from_secs(60);
const READ_LIMIT: u32 = 600;
const WRITE_LIMIT: u32 = 60;
const MAX_RATE_ENTRIES: usize = 10_000;
const OVERFLOW_KEY: &str = "peer:overflow";
const SSE_LIMIT_PER_USER: u8 = 4;

#[derive(Debug, Serialize)]
pub struct OperationalSnapshot {
    pub providers: ProviderOperations,
    pub generation_jobs: GenerationJobOperations,
    pub reviews: ReviewOperations,
    pub rights: RightsOperations,
    pub learning: LearningOperations,
    pub workspaces: WorkspaceOperations,
}

#[derive(Debug, Serialize)]
pub struct ProviderOperations {
    pub unhealthy: i64,
    pub unverified: i64,
    pub disabled: i64,
}

#[derive(Debug, Serialize)]
pub struct GenerationJobOperations {
    pub queued: i64,
    pub running: i64,
    pub expired_leases: i64,
    pub failed: i64,
    pub blocked: i64,
}

#[derive(Debug, Serialize)]
pub struct ReviewOperations {
    pub ai_pending: i64,
    pub human_pending: i64,
    pub rights_pending: i64,
    pub pilot_pending: i64,
    pub removal_pending: i64,
}

#[derive(Debug, Serialize)]
pub struct RightsOperations {
    pub publication_blockers: i64,
}

#[derive(Debug, Serialize)]
pub struct LearningOperations {
    pub active_projects: i64,
    pub stalled_projects: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceOperations {
    pub queued: i64,
    pub running: i64,
    pub expired_leases: i64,
    pub failed: i64,
}

#[derive(FromRow)]
struct OperationalSnapshotRow {
    provider_unhealthy: i64,
    provider_unverified: i64,
    provider_disabled: i64,
    jobs_queued: i64,
    jobs_running: i64,
    jobs_expired: i64,
    jobs_failed: i64,
    jobs_blocked: i64,
    reviews_ai: i64,
    reviews_human: i64,
    reviews_rights: i64,
    reviews_pilot: i64,
    reviews_removal: i64,
    rights_blockers: i64,
    learning_active: i64,
    learning_stalled: i64,
    workspaces_queued: i64,
    workspaces_running: i64,
    workspaces_expired: i64,
    workspaces_failed: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct PublicationViolation {
    pub resource_kind: String,
    pub resource_id: String,
    pub reason: String,
}

pub async fn publication_violations(
    pool: &PgPool,
) -> Result<Vec<PublicationViolation>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT 'content_review'::text AS resource_kind, item.id::text AS resource_id,
               'exact revision lacks approved commercial and redistribution rights'::text AS reason
        FROM content_review_items item
        WHERE item.state IN ('approved', 'published')
          AND NOT EXISTS (
            SELECT 1
            FROM content_provenance_records provenance
            JOIN content_rights_review_receipts rights
              ON rights.review_item_id = provenance.review_item_id
             AND rights.revision_number = provenance.revision_number
             AND rights.provenance_id = provenance.id
             AND rights.provenance_hash = provenance.provenance_hash
            WHERE provenance.review_item_id = item.id
              AND provenance.revision_number = item.revision_number
              AND provenance.artifact_hash = item.artifact_hash
              AND provenance.commercial_use_allowed
              AND provenance.redistribution_allowed
              AND rights.decision = 'approve'
              AND rights.commercial_use_allowed
              AND rights.redistribution_allowed
          )
        ORDER BY resource_kind, resource_id
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn operational_snapshot(pool: &PgPool) -> Result<OperationalSnapshot, sqlx::Error> {
    let row = sqlx::query_as::<_, OperationalSnapshotRow>(
        r#"
        SELECT
          (SELECT count(*) FROM content_provider_configs WHERE archived_at IS NULL AND health_status='unhealthy') AS provider_unhealthy,
          (SELECT count(*) FROM content_provider_configs WHERE archived_at IS NULL AND health_status='unverified') AS provider_unverified,
          (SELECT count(*) FROM content_provider_configs WHERE archived_at IS NULL AND NOT enabled) AS provider_disabled,
          (SELECT count(*) FROM content_generation_jobs WHERE status='queued') AS jobs_queued,
          (SELECT count(*) FROM content_generation_jobs WHERE status='leased') AS jobs_running,
          (SELECT count(*) FROM content_generation_jobs WHERE status='leased' AND lease_expires_at<=now()) AS jobs_expired,
          (SELECT count(*) FROM content_generation_jobs WHERE status='failed') AS jobs_failed,
          (SELECT count(*) FROM content_generation_jobs WHERE status IN ('blocked_disabled','blocked_missing_credential')) AS jobs_blocked,
          (SELECT count(*) FROM content_review_items WHERE state='ai_review_pending') AS reviews_ai,
          (SELECT count(*) FROM content_review_items WHERE state='human_review_pending') AS reviews_human,
          (SELECT count(*) FROM content_review_items WHERE state='rights_review_pending') AS reviews_rights,
          (SELECT count(*) FROM content_review_items WHERE state='pilot_pending') AS reviews_pilot,
          (SELECT count(*) FROM content_review_items WHERE state='removal_pending') AS reviews_removal,
          (SELECT count(*) FROM content_review_items item WHERE item.state IN ('approved','published') AND NOT EXISTS (
             SELECT 1 FROM content_provenance_records provenance
             JOIN content_rights_review_receipts rights ON rights.review_item_id=provenance.review_item_id
               AND rights.revision_number=provenance.revision_number AND rights.provenance_id=provenance.id
               AND rights.provenance_hash=provenance.provenance_hash
             WHERE provenance.review_item_id=item.id AND provenance.revision_number=item.revision_number
               AND provenance.artifact_hash=item.artifact_hash AND provenance.commercial_use_allowed
               AND provenance.redistribution_allowed AND rights.decision='approve'
               AND rights.commercial_use_allowed AND rights.redistribution_allowed)) AS rights_blockers,
          (SELECT count(*) FROM learner_projects WHERE status='active') AS learning_active,
          (SELECT count(*) FROM learner_projects project
             WHERE project.status='active' AND project.created_at<now()-interval '7 days'
               AND NOT EXISTS (SELECT 1 FROM project_learning_events event WHERE event.project_id=project.id AND event.created_at>=now()-interval '7 days')) AS learning_stalled,
          (SELECT count(*) FROM workspace_runs WHERE status='queued') AS workspaces_queued,
          (SELECT count(*) FROM workspace_runs WHERE status IN ('leased','running')) AS workspaces_running,
          (SELECT count(*) FROM workspace_runs WHERE status IN ('leased','running') AND lease_expires_at<=now()) AS workspaces_expired,
          (SELECT count(*) FROM workspace_runs WHERE status='failed') AS workspaces_failed
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(OperationalSnapshot {
        providers: ProviderOperations {
            unhealthy: row.provider_unhealthy,
            unverified: row.provider_unverified,
            disabled: row.provider_disabled,
        },
        generation_jobs: GenerationJobOperations {
            queued: row.jobs_queued,
            running: row.jobs_running,
            expired_leases: row.jobs_expired,
            failed: row.jobs_failed,
            blocked: row.jobs_blocked,
        },
        reviews: ReviewOperations {
            ai_pending: row.reviews_ai,
            human_pending: row.reviews_human,
            rights_pending: row.reviews_rights,
            pilot_pending: row.reviews_pilot,
            removal_pending: row.reviews_removal,
        },
        rights: RightsOperations {
            publication_blockers: row.rights_blockers,
        },
        learning: LearningOperations {
            active_projects: row.learning_active,
            stalled_projects: row.learning_stalled,
        },
        workspaces: WorkspaceOperations {
            queued: row.workspaces_queued,
            running: row.workspaces_running,
            expired_leases: row.workspaces_expired,
            failed: row.workspaces_failed,
        },
    })
}

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
