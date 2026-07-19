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
const ACTION_ITEM_LIMIT: i64 = 20;

#[derive(Debug, Serialize, FromRow)]
pub struct OperationalActionItem {
    pub resource_id: String,
    pub state: String,
    pub reason: String,
    pub age_seconds: i64,
}

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
    pub action_items: Vec<OperationalActionItem>,
}

#[derive(Debug, Serialize)]
pub struct GenerationJobOperations {
    pub queued: i64,
    pub running: i64,
    pub expired_leases: i64,
    pub failed: i64,
    pub blocked: i64,
    pub action_items: Vec<OperationalActionItem>,
}

#[derive(Debug, Serialize)]
pub struct ReviewOperations {
    pub ai_pending: i64,
    pub human_pending: i64,
    pub rights_pending: i64,
    pub pilot_pending: i64,
    pub removal_pending: i64,
    pub action_items: Vec<OperationalActionItem>,
}

#[derive(Debug, Serialize)]
pub struct RightsOperations {
    pub publication_blockers: i64,
    pub action_items: Vec<OperationalActionItem>,
}

#[derive(Debug, Serialize)]
pub struct LearningOperations {
    pub active_projects: i64,
    pub stalled_projects: i64,
    pub action_items: Vec<OperationalActionItem>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceOperations {
    pub queued: i64,
    pub running: i64,
    pub expired_leases: i64,
    pub failed: i64,
    pub action_items: Vec<OperationalActionItem>,
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
    learning_active: i64,
    learning_stalled: i64,
    workspaces_queued: i64,
    workspaces_running: i64,
    workspaces_expired: i64,
    workspaces_failed: i64,
}

#[derive(FromRow)]
struct PublicationBlockerRow {
    resource_id: String,
    state: String,
    reason: String,
    age_seconds: i64,
    blocker_count: i64,
}

pub struct PublicationGateStatus {
    pub blocker_count: i64,
    pub blockers: Vec<OperationalActionItem>,
}

pub async fn publication_gate_status(pool: &PgPool) -> Result<PublicationGateStatus, sqlx::Error> {
    let rows = sqlx::query_as::<_, PublicationBlockerRow>(
        r#"
        WITH evaluated AS (
          SELECT item.id::text AS resource_id, item.state,
                 greatest(0, extract(epoch FROM now()-item.updated_at)::bigint) AS age_seconds,
                 CASE
                   WHEN item.state NOT IN ('pilot_pending','approved','published','unpublished')
                     THEN 'rights_not_approved'
                   WHEN provenance.id IS NULL AND EXISTS (
                     SELECT 1 FROM content_provenance_records stale
                     WHERE stale.review_item_id=item.id
                   ) THEN 'stale_revision_evidence'
                   WHEN provenance.id IS NULL THEN 'missing_provenance'
                   WHEN provenance.legal_status<>'pending' THEN 'rights_not_approved'
                   WHEN NOT provenance.commercial_use_allowed THEN 'commercial_use_denied'
                   WHEN NOT provenance.redistribution_allowed THEN 'redistribution_denied'
                   WHEN EXISTS (
                     SELECT 1 FROM content_rights_review_receipts rights
                     WHERE rights.review_item_id=item.id
                       AND rights.revision_number=item.revision_number
                       AND rights.provenance_id=provenance.id
                       AND rights.provenance_hash=provenance.provenance_hash
                       AND rights.decision='approve'
                       AND rights.commercial_use_allowed
                       AND rights.redistribution_allowed
                   ) THEN NULL
                   WHEN EXISTS (
                     SELECT 1 FROM content_rights_review_receipts rights
                     WHERE rights.review_item_id=item.id
                       AND rights.revision_number=item.revision_number
                       AND rights.provenance_id=provenance.id
                       AND rights.provenance_hash=provenance.provenance_hash
                       AND rights.decision='approve'
                       AND NOT rights.commercial_use_allowed
                   ) THEN 'commercial_use_denied'
                   WHEN EXISTS (
                     SELECT 1 FROM content_rights_review_receipts rights
                     WHERE rights.review_item_id=item.id
                       AND rights.revision_number=item.revision_number
                       AND rights.provenance_id=provenance.id
                       AND rights.provenance_hash=provenance.provenance_hash
                       AND rights.decision='approve'
                       AND NOT rights.redistribution_allowed
                   ) THEN 'redistribution_denied'
                   WHEN EXISTS (
                     SELECT 1 FROM content_rights_review_receipts rights
                     WHERE rights.review_item_id=item.id
                       AND rights.revision_number=item.revision_number
                       AND rights.provenance_id=provenance.id
                       AND rights.provenance_hash=provenance.provenance_hash
                   ) THEN 'rights_not_approved'
                   WHEN EXISTS (
                     SELECT 1 FROM content_rights_review_receipts stale
                     WHERE stale.review_item_id=item.id
                   ) THEN 'stale_revision_evidence'
                   ELSE 'missing_rights_approval'
                 END AS reason
          FROM content_review_items item
          LEFT JOIN content_provenance_records provenance
            ON provenance.review_item_id=item.id
           AND provenance.revision_number=item.revision_number
           AND provenance.artifact_hash=item.artifact_hash
          WHERE item.state NOT IN ('rejected','removal_completed')
        ), blockers AS (
          SELECT * FROM evaluated WHERE reason IS NOT NULL
        )
        SELECT resource_id, state, reason, age_seconds, count(*) OVER() AS blocker_count
        FROM blockers
        ORDER BY age_seconds DESC, resource_id
        LIMIT $1
        "#,
    )
    .bind(ACTION_ITEM_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(PublicationGateStatus {
        blocker_count: rows.first().map_or(0, |row| row.blocker_count),
        blockers: rows
            .into_iter()
            .map(|row| OperationalActionItem {
                resource_id: row.resource_id,
                state: row.state,
                reason: row.reason,
                age_seconds: row.age_seconds,
            })
            .collect(),
    })
}

#[derive(FromRow)]
struct OperationalActionRow {
    category: String,
    resource_id: String,
    state: String,
    reason: String,
    age_seconds: i64,
}

async fn operational_action_items(pool: &PgPool) -> Result<Vec<OperationalActionRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        WITH providers AS (
          SELECT 'providers'::text category, id::text resource_id,
                 CASE WHEN NOT enabled THEN 'disabled' ELSE health_status END state,
                 CASE WHEN NOT enabled THEN 'provider_disabled' ELSE 'provider_'||health_status END reason,
                 greatest(0,extract(epoch FROM now()-coalesce(last_health_at,created_at))::bigint) age_seconds
          FROM content_provider_configs
          WHERE archived_at IS NULL AND (NOT enabled OR health_status IN ('unhealthy','unverified'))
          ORDER BY coalesce(last_health_at,created_at) LIMIT $1
        ), jobs AS (
          SELECT 'generation_jobs'::text category, id::text resource_id, status state,
                 CASE WHEN status='leased' THEN 'expired_lease'
                      WHEN status='blocked_disabled' THEN 'provider_disabled'
                      WHEN status='blocked_missing_credential' THEN 'missing_credential'
                      ELSE 'generation_failed' END reason,
                 greatest(0,extract(epoch FROM now()-updated_at)::bigint) age_seconds
          FROM content_generation_jobs
          WHERE (status='leased' AND lease_expires_at<=now())
             OR status IN ('failed','blocked_disabled','blocked_missing_credential')
          ORDER BY updated_at LIMIT $1
        ), reviews AS (
          SELECT 'reviews'::text category, id::text resource_id, state, state reason,
                 greatest(0,extract(epoch FROM now()-updated_at)::bigint) age_seconds
          FROM content_review_items
          WHERE state IN ('ai_review_pending','human_review_pending','rights_review_pending','pilot_pending','removal_pending')
          ORDER BY updated_at LIMIT $1
        ), learning AS (
          SELECT 'learning'::text category, project.id::text resource_id, project.status state,
                 'no_learning_event_7d'::text reason,
                 greatest(0,extract(epoch FROM now()-project.created_at)::bigint) age_seconds
          FROM learner_projects project
          WHERE project.status='active' AND project.created_at<now()-interval '7 days'
            AND NOT EXISTS (SELECT 1 FROM project_learning_events event
                            WHERE event.project_id=project.id AND event.created_at>=now()-interval '7 days')
          ORDER BY project.created_at LIMIT $1
        ), workspaces AS (
          SELECT 'workspaces'::text category, id::text resource_id, status state,
                 CASE WHEN status IN ('leased','running') THEN 'expired_lease' ELSE 'workspace_failed' END reason,
                 greatest(0,extract(epoch FROM now()-created_at)::bigint) age_seconds
          FROM workspace_runs
          WHERE (status IN ('leased','running') AND lease_expires_at<=now()) OR status='failed'
          ORDER BY created_at LIMIT $1
        )
        SELECT * FROM providers UNION ALL SELECT * FROM jobs UNION ALL SELECT * FROM reviews
        UNION ALL SELECT * FROM learning UNION ALL SELECT * FROM workspaces
        "#,
    )
    .bind(ACTION_ITEM_LIMIT)
    .fetch_all(pool)
    .await
}

pub async fn operational_snapshot(pool: &PgPool) -> Result<OperationalSnapshot, sqlx::Error> {
    let publication = publication_gate_status(pool).await?;
    let mut provider_items = Vec::new();
    let mut generation_items = Vec::new();
    let mut review_items = Vec::new();
    let mut learning_items = Vec::new();
    let mut workspace_items = Vec::new();
    for row in operational_action_items(pool).await? {
        let item = OperationalActionItem {
            resource_id: row.resource_id,
            state: row.state,
            reason: row.reason,
            age_seconds: row.age_seconds,
        };
        match row.category.as_str() {
            "providers" => provider_items.push(item),
            "generation_jobs" => generation_items.push(item),
            "reviews" => review_items.push(item),
            "learning" => learning_items.push(item),
            "workspaces" => workspace_items.push(item),
            _ => {}
        }
    }
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
            action_items: provider_items,
        },
        generation_jobs: GenerationJobOperations {
            queued: row.jobs_queued,
            running: row.jobs_running,
            expired_leases: row.jobs_expired,
            failed: row.jobs_failed,
            blocked: row.jobs_blocked,
            action_items: generation_items,
        },
        reviews: ReviewOperations {
            ai_pending: row.reviews_ai,
            human_pending: row.reviews_human,
            rights_pending: row.reviews_rights,
            pilot_pending: row.reviews_pilot,
            removal_pending: row.reviews_removal,
            action_items: review_items,
        },
        rights: RightsOperations {
            publication_blockers: publication.blocker_count,
            action_items: publication.blockers,
        },
        learning: LearningOperations {
            active_projects: row.learning_active,
            stalled_projects: row.learning_stalled,
            action_items: learning_items,
        },
        workspaces: WorkspaceOperations {
            queued: row.workspaces_queued,
            running: row.workspaces_running,
            expired_leases: row.workspaces_expired,
            failed: row.workspaces_failed,
            action_items: workspace_items,
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
