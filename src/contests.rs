use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug)]
pub enum ContestError {
    Auth(AuthError),
    InvalidInput(&'static str),
    TermsRequired,
    NotFound,
    RegistrationRequired,
    RoleForbidden,
    NotEnded,
    Database(sqlx::Error),
}

impl IntoResponse for ContestError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(error) => return error.into_response(),
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, "invalid_input", message),
            Self::TermsRequired => (
                StatusCode::FORBIDDEN,
                "terms_required",
                "대회 참가 전에 이용약관에 동의해 주세요",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "contest_not_found",
                "대회를 찾을 수 없습니다",
            ),
            Self::RegistrationRequired => (
                StatusCode::FORBIDDEN,
                "contest_registration_required",
                "이 대회에 참가 등록이 필요합니다",
            ),
            Self::RoleForbidden => (
                StatusCode::FORBIDDEN,
                "contest_manager_required",
                "대회 관리자 권한이 필요합니다",
            ),
            Self::NotEnded => (
                StatusCode::CONFLICT,
                "contest_not_ended",
                "대회 종료 후에만 레이팅을 확정할 수 있습니다",
            ),
            Self::Database(error) => {
                tracing::error!(%error, "대회 데이터베이스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "요청을 처리하지 못했습니다",
                )
            }
        };
        (
            status,
            Json(serde_json::json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

impl From<AuthError> for ContestError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for ContestError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn require_terms(state: &AppState, user_id: Uuid) -> Result<(), ContestError> {
    let accepted: bool = sqlx::query_scalar(
        "SELECT terms_accepted_at IS NOT NULL FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AuthError::Unauthorized)?;
    if !accepted {
        return Err(ContestError::TermsRequired);
    }
    Ok(())
}

async fn require_manager(state: &AppState, user_id: Uuid) -> Result<(), ContestError> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role IN ('CONTEST_MANAGER', 'ADMIN'))",
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !allowed {
        return Err(ContestError::RoleForbidden);
    }
    Ok(())
}

async fn require_contest_view(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
) -> Result<(), ContestError> {
    let contest: (Uuid, String) = sqlx::query_as(
        "SELECT id, visibility FROM contests WHERE slug = $1 AND status = 'published'",
    )
    .bind(slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ContestError::NotFound)?;
    if contest.1 == "public" {
        return Ok(());
    }
    let user_id = crate::auth::authenticated_user_id(state, headers)
        .await
        .map_err(|_| ContestError::NotFound)?;
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM contest_registrations WHERE contest_id = $1 AND user_id = $2 AND status = 'active')",
    )
    .bind(contest.0)
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    if !registered {
        return Err(ContestError::NotFound);
    }
    Ok(())
}

#[derive(Debug, Serialize, FromRow)]
pub struct ContestListItem {
    slug: String,
    title: String,
    description: String,
    visibility: String,
    scoring_mode: String,
    #[serde(with = "time::serde::rfc3339")]
    starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    freezes_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    ends_at: OffsetDateTime,
    state: String,
    participant_count: i64,
    problem_count: i64,
}

#[derive(Debug, Serialize)]
pub struct ContestListResponse {
    items: Vec<ContestListItem>,
}

pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<ContestListResponse>, ContestError> {
    let items = sqlx::query_as::<_, ContestListItem>(
        r#"
        SELECT contest.slug, contest.title_ko AS title, contest.description_ko AS description,
               contest.visibility, contest.scoring_mode, contest.starts_at, contest.freezes_at,
               contest.ends_at,
               CASE WHEN now() < contest.starts_at THEN 'scheduled'
                    WHEN now() < contest.ends_at THEN 'running' ELSE 'ended' END AS state,
               (SELECT COUNT(*) FROM contest_registrations registration WHERE registration.contest_id = contest.id AND registration.status = 'active') AS participant_count,
               (SELECT COUNT(*) FROM contest_problems problem WHERE problem.contest_id = contest.id) AS problem_count
        FROM contests contest
        WHERE contest.status = 'published' AND contest.visibility = 'public'
        ORDER BY contest.starts_at DESC, contest.id DESC
        "#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(ContestListResponse { items }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct ContestProblemView {
    label: String,
    slug: String,
    title: String,
    difficulty: i16,
    points: i32,
}

#[derive(Debug, Serialize)]
pub struct ContestDetailResponse {
    contest: ContestListItem,
    problems: Vec<ContestProblemView>,
    registered: bool,
}

async fn contest_by_slug(pool: &PgPool, slug: &str) -> Result<ContestListItem, ContestError> {
    sqlx::query_as::<_, ContestListItem>(
        r#"
        SELECT contest.slug, contest.title_ko AS title, contest.description_ko AS description,
               contest.visibility, contest.scoring_mode, contest.starts_at, contest.freezes_at,
               contest.ends_at,
               CASE WHEN now() < contest.starts_at THEN 'scheduled'
                    WHEN now() < contest.ends_at THEN 'running' ELSE 'ended' END AS state,
               (SELECT COUNT(*) FROM contest_registrations registration WHERE registration.contest_id = contest.id AND registration.status = 'active') AS participant_count,
               (SELECT COUNT(*) FROM contest_problems problem WHERE problem.contest_id = contest.id) AS problem_count
        FROM contests contest WHERE contest.slug = $1 AND contest.status = 'published'
        "#,
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?
    .ok_or(ContestError::NotFound)
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<ContestDetailResponse>, ContestError> {
    require_contest_view(&state, &headers, &slug).await?;
    let contest = contest_by_slug(state.pool(), &slug).await?;
    let registered = if let Ok(user_id) = crate::auth::authenticated_user_id(&state, &headers).await
    {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM contest_registrations registration JOIN contests contest ON contest.id = registration.contest_id WHERE contest.slug = $1 AND registration.user_id = $2 AND registration.status = 'active')",
        )
        .bind(&slug)
        .bind(user_id)
        .fetch_one(state.pool())
        .await?
    } else {
        false
    };
    let problems = sqlx::query_as::<_, ContestProblemView>(
        r#"
        SELECT contest_problem.label, problem.slug, problem.title_ko AS title,
               problem.difficulty, contest_problem.points
        FROM contest_problems contest_problem
        JOIN contests contest ON contest.id = contest_problem.contest_id
        JOIN problems problem ON problem.id = contest_problem.problem_id
        WHERE contest.slug = $1 ORDER BY contest_problem.position
        "#,
    )
    .bind(slug)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(ContestDetailResponse {
        contest,
        problems,
        registered,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinRequest {
    join_code: Option<String>,
}

pub async fn join(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(request): Json<JoinRequest>,
) -> Result<StatusCode, ContestError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_terms(&state, user_id).await?;
    let contest: (Uuid, String, Option<Uuid>, Option<Vec<u8>>, OffsetDateTime) = sqlx::query_as(
        "SELECT id, visibility, organization_id, join_code_hash, ends_at FROM contests WHERE slug = $1 AND status = 'published'",
    )
    .bind(slug)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ContestError::NotFound)?;
    if OffsetDateTime::now_utc() >= contest.4 {
        return Err(ContestError::InvalidInput(
            "종료된 대회에는 참가할 수 없습니다",
        ));
    }
    match contest.1.as_str() {
        "private" => {
            let supplied = request
                .join_code
                .filter(|code| (4..=64).contains(&code.len()))
                .ok_or(ContestError::RegistrationRequired)?;
            let digest = Sha256::digest(supplied.as_bytes());
            if !bool::from(
                contest
                    .3
                    .as_deref()
                    .ok_or(ContestError::RegistrationRequired)?
                    .ct_eq(&digest),
            ) {
                return Err(ContestError::RegistrationRequired);
            }
        }
        "organization" => {
            let member: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM organization_memberships WHERE organization_id = $1 AND user_id = $2)",
            )
            .bind(contest.2)
            .bind(user_id)
            .fetch_one(state.pool())
            .await?;
            if !member {
                return Err(ContestError::RegistrationRequired);
            }
        }
        _ => {}
    }
    sqlx::query(
        r#"
        INSERT INTO contest_registrations (contest_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (contest_id, user_id) DO UPDATE SET status = 'active'
        "#,
    )
    .bind(contest.0)
    .bind(user_id)
    .execute(state.pool())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, FromRow)]
pub struct ScoreboardEntry {
    rank: i64,
    user_id: Uuid,
    handle: String,
    display_name: String,
    solved: i64,
    penalty: i64,
    score: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ScoreboardCell {
    user_id: Uuid,
    problem_label: String,
    solved: bool,
    attempts: i64,
    penalty_minutes: i64,
    score: i64,
    frozen_attempts: i64,
}

#[derive(Debug, Serialize)]
pub struct ScoreboardResponse {
    scoring_mode: String,
    frozen: bool,
    #[serde(with = "time::serde::rfc3339")]
    generated_at: OffsetDateTime,
    entries: Vec<ScoreboardEntry>,
    cells: Vec<ScoreboardCell>,
}

async fn scoreboard_data(pool: &PgPool, slug: &str) -> Result<ScoreboardResponse, ContestError> {
    let contest: (Uuid, String, OffsetDateTime, Option<OffsetDateTime>, OffsetDateTime) =
        sqlx::query_as(
            "SELECT id, scoring_mode, starts_at, freezes_at, ends_at FROM contests WHERE slug = $1 AND status = 'published'",
        )
        .bind(slug)
        .fetch_optional(pool)
        .await?
        .ok_or(ContestError::NotFound)?;
    let now = OffsetDateTime::now_utc();
    let frozen = contest
        .3
        .is_some_and(|freeze| now >= freeze && now < contest.4);
    let cutoff = if frozen {
        contest.3.unwrap_or(now)
    } else {
        now.min(contest.4)
    };
    let cells = sqlx::query_as::<_, ScoreboardCell>(
        r#"
        WITH participants AS (
            SELECT user_id FROM contest_registrations WHERE contest_id = $1 AND status = 'active'
        ), grid AS (
            SELECT participant.user_id, problem.problem_id, problem.label, problem.points
            FROM participants participant CROSS JOIN contest_problems problem
            WHERE problem.contest_id = $1
        ), first_accept AS (
            SELECT user_id, problem_id, MIN(created_at) AS accepted_at
            FROM submissions
            WHERE contest_id = $1 AND run_kind = 'formal' AND status = 'ACCEPTED'
              AND created_at >= $2 AND created_at <= $3
            GROUP BY user_id, problem_id
        )
        SELECT grid.user_id, grid.label AS problem_label,
               accepted.accepted_at IS NOT NULL AS solved,
               COUNT(submission.id) FILTER (
                   WHERE submission.created_at <= COALESCE(accepted.accepted_at, $3)
                     AND submission.status NOT IN ('QUEUED', 'COMPILING', 'RUNNING', 'CANCELLED')
               ) AS attempts,
               CASE WHEN accepted.accepted_at IS NULL THEN 0
                    ELSE FLOOR(EXTRACT(EPOCH FROM (accepted.accepted_at - $2)) / 60)::bigint
                         + 20 * COUNT(submission.id) FILTER (
                             WHERE submission.created_at < accepted.accepted_at
                               AND submission.status <> 'ACCEPTED'
                               AND submission.status NOT IN ('QUEUED', 'COMPILING', 'RUNNING', 'CANCELLED')
                         ) END AS penalty_minutes,
               COALESCE(MAX(submission.score) FILTER (WHERE submission.created_at <= $3), 0)::bigint AS score,
               COUNT(submission.id) FILTER (WHERE submission.created_at > $3 AND submission.created_at <= $4)::bigint AS frozen_attempts
        FROM grid
        LEFT JOIN first_accept accepted ON accepted.user_id = grid.user_id AND accepted.problem_id = grid.problem_id
        LEFT JOIN submissions submission ON submission.contest_id = $1 AND submission.user_id = grid.user_id
            AND submission.problem_id = grid.problem_id AND submission.run_kind = 'formal'
            AND submission.created_at >= $2 AND submission.created_at <= $4
        GROUP BY grid.user_id, grid.label, grid.points, accepted.accepted_at
        ORDER BY grid.user_id, grid.label
        "#,
    )
    .bind(contest.0)
    .bind(contest.2)
    .bind(cutoff)
    .bind(now.min(contest.4))
    .fetch_all(pool)
    .await?;

    let entries = if contest.1 == "score" {
        sqlx::query_as::<_, ScoreboardEntry>(
            r#"
            WITH scores AS (
                SELECT registration.user_id, COALESCE(SUM(best.score), 0)::bigint AS score
                FROM contest_registrations registration
                LEFT JOIN (
                    SELECT user_id, problem_id, MAX(score)::bigint AS score
                    FROM submissions WHERE contest_id = $1 AND run_kind = 'formal' AND created_at <= $2
                    GROUP BY user_id, problem_id
                ) best ON best.user_id = registration.user_id
                WHERE registration.contest_id = $1 AND registration.status = 'active'
                GROUP BY registration.user_id
            )
            SELECT ROW_NUMBER() OVER (ORDER BY scores.score DESC, user_account.id)::bigint AS rank,
                   user_account.id AS user_id, user_account.handle, user_account.display_name,
                   0::bigint AS solved, 0::bigint AS penalty, scores.score
            FROM scores JOIN users user_account ON user_account.id = scores.user_id
            ORDER BY rank
            "#,
        )
        .bind(contest.0)
        .bind(cutoff)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, ScoreboardEntry>(
            r#"
            WITH first_accept AS (
                SELECT user_id, problem_id, MIN(created_at) AS accepted_at
                FROM submissions WHERE contest_id = $1 AND run_kind = 'formal'
                  AND status = 'ACCEPTED' AND created_at >= $2 AND created_at <= $3
                GROUP BY user_id, problem_id
            ), participant_score AS (
                SELECT registration.user_id, COUNT(accepted.problem_id)::bigint AS solved,
                       COALESCE(SUM(
                           FLOOR(EXTRACT(EPOCH FROM (accepted.accepted_at - $2)) / 60)
                           + 20 * (SELECT COUNT(*) FROM submissions wrong
                                   WHERE wrong.contest_id = $1 AND wrong.user_id = registration.user_id
                                     AND wrong.problem_id = accepted.problem_id
                                     AND wrong.created_at < accepted.accepted_at
                                     AND wrong.status <> 'ACCEPTED'
                                     AND wrong.status NOT IN ('QUEUED', 'COMPILING', 'RUNNING', 'CANCELLED'))
                       ), 0)::bigint AS penalty
                FROM contest_registrations registration
                LEFT JOIN first_accept accepted ON accepted.user_id = registration.user_id
                WHERE registration.contest_id = $1 AND registration.status = 'active'
                GROUP BY registration.user_id
            )
            SELECT ROW_NUMBER() OVER (ORDER BY score.solved DESC, score.penalty, user_account.id)::bigint AS rank,
                   user_account.id AS user_id, user_account.handle, user_account.display_name,
                   score.solved, score.penalty, score.solved * 100 AS score
            FROM participant_score score JOIN users user_account ON user_account.id = score.user_id
            ORDER BY rank
            "#,
        )
        .bind(contest.0)
        .bind(contest.2)
        .bind(cutoff)
        .fetch_all(pool)
        .await?
    };
    Ok(ScoreboardResponse {
        scoring_mode: contest.1,
        frozen,
        generated_at: now,
        entries,
        cells,
    })
}

pub async fn scoreboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<ScoreboardResponse>, ContestError> {
    require_contest_view(&state, &headers, &slug).await?;
    Ok(Json(scoreboard_data(state.pool(), &slug).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateContestRequest {
    slug: String,
    title: String,
    description: String,
    visibility: String,
    organization_id: Option<Uuid>,
    join_code: Option<String>,
    scoring_mode: String,
    #[serde(with = "time::serde::rfc3339")]
    starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    freezes_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    ends_at: OffsetDateTime,
    problem_slugs: Vec<String>,
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateContestRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ContestError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_manager(&state, user_id).await?;
    if request.title.chars().count() > 120
        || request.title.trim().is_empty()
        || request.description.chars().count() > 2000
        || !(1..=26).contains(&request.problem_slugs.len())
        || !matches!(request.scoring_mode.as_str(), "icpc" | "score")
        || !matches!(
            request.visibility.as_str(),
            "public" | "private" | "organization"
        )
        || request.ends_at <= request.starts_at
        || request
            .freezes_at
            .is_some_and(|freeze| freeze <= request.starts_at || freeze >= request.ends_at)
        || (request.visibility == "private"
            && request
                .join_code
                .as_ref()
                .is_none_or(|code| !(4..=64).contains(&code.len())))
        || (request.visibility == "organization") != request.organization_id.is_some()
    {
        return Err(ContestError::InvalidInput("대회 설정을 확인해 주세요"));
    }
    let join_code_hash = request
        .join_code
        .as_ref()
        .map(|code| Sha256::digest(code.as_bytes()).to_vec());
    let mut transaction = state.pool().begin().await?;
    let contest_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO contests (
            slug, title_ko, description_ko, visibility, organization_id, join_code_hash,
            scoring_mode, starts_at, freezes_at, ends_at, created_by
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id
        "#,
    )
    .bind(&request.slug)
    .bind(request.title.trim())
    .bind(request.description.trim())
    .bind(&request.visibility)
    .bind(request.organization_id)
    .bind(join_code_hash)
    .bind(&request.scoring_mode)
    .bind(request.starts_at)
    .bind(request.freezes_at)
    .bind(request.ends_at)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    for (index, problem_slug) in request.problem_slugs.iter().enumerate() {
        let label = ((b'A' + index as u8) as char).to_string();
        let inserted = sqlx::query(
            r#"
            INSERT INTO contest_problems (contest_id, problem_id, position, label)
            SELECT $1, id, $2, $3 FROM problems WHERE slug = $4 AND status = 'published'
            "#,
        )
        .bind(contest_id)
        .bind(index as i32 + 1)
        .bind(label)
        .bind(problem_slug)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(ContestError::InvalidInput(
                "공개된 문제만 대회에 추가할 수 있습니다",
            ));
        }
    }
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id) VALUES ($1, 'contest.created', 'contest', $2)",
    )
    .bind(user_id)
    .bind(contest_id.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": contest_id, "slug": request.slug})),
    ))
}

pub async fn finalize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<serde_json::Value>, ContestError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    require_manager(&state, user_id).await?;
    let contest: (Uuid, OffsetDateTime) =
        sqlx::query_as("SELECT id, ends_at FROM contests WHERE slug = $1 AND status = 'published'")
            .bind(&slug)
            .fetch_optional(state.pool())
            .await?
            .ok_or(ContestError::NotFound)?;
    if OffsetDateTime::now_utc() < contest.1 {
        return Err(ContestError::NotEnded);
    }
    let scoreboard = scoreboard_data(state.pool(), &slug).await?;
    let mut transaction = state.pool().begin().await?;
    let mut finalized = 0;
    for entry in scoreboard.entries {
        let before: i32 = sqlx::query_scalar(
            "SELECT rating FROM user_mastery WHERE user_id = $1 AND axis = 'contest' FOR UPDATE",
        )
        .bind(entry.user_id)
        .fetch_one(&mut *transaction)
        .await?;
        let proposed = (40 - (entry.rank as i32 - 1) * 10).clamp(-30, 40);
        let after = (before + proposed).clamp(0, 1000);
        let delta = after - before;
        let inserted = sqlx::query(
            r#"
            INSERT INTO contest_rating_history (
                contest_id, user_id, rank, rating_before, rating_delta, rating_after
            ) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING
            "#,
        )
        .bind(contest.0)
        .bind(entry.user_id)
        .bind(entry.rank as i32)
        .bind(before)
        .bind(delta)
        .bind(after)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() == 1 {
            sqlx::query(
                r#"
                UPDATE user_mastery SET rating = $2,
                    tier = CASE WHEN $2 >= 800 THEN '마스터' WHEN $2 >= 600 THEN '플래티넘'
                                WHEN $2 >= 400 THEN '골드' WHEN $2 >= 200 THEN '실버'
                                WHEN $2 >= 50 THEN '브론즈' ELSE '새싹' END,
                    updated_at = now()
                WHERE user_id = $1 AND axis = 'contest'
                "#,
            )
            .bind(entry.user_id)
            .bind(after)
            .execute(&mut *transaction)
            .await?;
            finalized += 1;
        }
    }
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, action, target_type, target_id, metadata) VALUES ($1, 'contest.rating.finalized', 'contest', $2, jsonb_build_object('participants', $3::integer))",
    )
    .bind(user_id)
    .bind(contest.0.to_string())
    .bind(finalized)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"finalized": finalized})))
}
