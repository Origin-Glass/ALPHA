use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

pub struct RewardSpec {
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub reward_key: String,
    pub source_kind: &'static str,
    pub source_id: Option<Uuid>,
    pub xp: i32,
    pub reason_ko: &'static str,
    pub axis: &'static str,
    pub mastery_points: i32,
    pub mastery_class: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RewardOutcome {
    pub awarded: bool,
    pub xp_awarded: i32,
}

#[derive(Debug, FromRow)]
struct QuestRow {
    id: Uuid,
    slug: String,
    cadence: String,
    target: i32,
    reward_xp: i32,
}

pub async fn apply_reward(
    transaction: &mut Transaction<'_, Postgres>,
    reward: RewardSpec,
) -> Result<RewardOutcome, sqlx::Error> {
    let inserted: Option<i32> = sqlx::query_scalar(
        r#"
        INSERT INTO xp_events (
            event_id, user_id, reward_key, source_kind, source_id, xp, reason_ko, season_id
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            (SELECT id FROM seasons WHERE status = 'active' AND starts_at <= now() AND ends_at > now() ORDER BY starts_at DESC LIMIT 1)
        )
        ON CONFLICT DO NOTHING
        RETURNING xp
        "#,
    )
    .bind(reward.event_id)
    .bind(reward.user_id)
    .bind(&reward.reward_key)
    .bind(reward.source_kind)
    .bind(reward.source_id)
    .bind(reward.xp)
    .bind(reward.reason_ko)
    .fetch_optional(&mut **transaction)
    .await?;
    if inserted.is_none() {
        return Ok(RewardOutcome {
            awarded: false,
            xp_awarded: 0,
        });
    }

    let independent = reward.mastery_class == "independent";
    let current_streak: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO progression_state (
            user_id, xp, level, current_streak, best_streak, last_meaningful_date,
            mastered_count, independent_mastered_count
        ) VALUES ($1, $2, 1 + ($2 / 500), 1, 1, (now() AT TIME ZONE 'Asia/Seoul')::date, 1, $3)
        ON CONFLICT (user_id) DO UPDATE
        SET xp = progression_state.xp + EXCLUDED.xp,
            level = 1 + ((progression_state.xp + EXCLUDED.xp) / 500)::integer,
            current_streak = CASE
                WHEN progression_state.last_meaningful_date = EXCLUDED.last_meaningful_date THEN progression_state.current_streak
                WHEN progression_state.last_meaningful_date = EXCLUDED.last_meaningful_date - 1 THEN progression_state.current_streak + 1
                WHEN progression_state.last_meaningful_date = EXCLUDED.last_meaningful_date - 2
                     AND progression_state.streak_protections > 0 THEN progression_state.current_streak + 1
                ELSE 1
            END,
            best_streak = GREATEST(
                progression_state.best_streak,
                CASE
                    WHEN progression_state.last_meaningful_date = EXCLUDED.last_meaningful_date THEN progression_state.current_streak
                    WHEN progression_state.last_meaningful_date IN (EXCLUDED.last_meaningful_date - 1, EXCLUDED.last_meaningful_date - 2) THEN progression_state.current_streak + 1
                    ELSE 1
                END
            ),
            streak_protections = CASE
                WHEN progression_state.last_meaningful_date = EXCLUDED.last_meaningful_date - 2
                     AND progression_state.streak_protections > 0 THEN progression_state.streak_protections - 1
                ELSE progression_state.streak_protections
            END,
            last_meaningful_date = EXCLUDED.last_meaningful_date,
            mastered_count = progression_state.mastered_count + 1,
            independent_mastered_count = progression_state.independent_mastered_count + $3,
            updated_at = now()
        RETURNING current_streak
        "#,
    )
    .bind(reward.user_id)
    .bind(reward.xp)
    .bind(i32::from(independent))
    .fetch_one(&mut **transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO mastery_events (event_id, user_id, axis, points, mastery_class)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(reward.event_id)
    .bind(reward.user_id)
    .bind(reward.axis)
    .bind(reward.mastery_points)
    .bind(reward.mastery_class)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO user_mastery (user_id, axis, rating, tier, evidence_count)
        VALUES (
            $1, $2, $3,
            CASE WHEN $3 >= 800 THEN '마스터' WHEN $3 >= 600 THEN '플래티넘'
                 WHEN $3 >= 400 THEN '골드' WHEN $3 >= 200 THEN '실버'
                 WHEN $3 >= 50 THEN '브론즈' ELSE '새싹' END,
            1
        )
        ON CONFLICT (user_id, axis) DO UPDATE
        SET rating = LEAST(1000, user_mastery.rating + EXCLUDED.rating),
            tier = CASE WHEN LEAST(1000, user_mastery.rating + EXCLUDED.rating) >= 800 THEN '마스터'
                        WHEN LEAST(1000, user_mastery.rating + EXCLUDED.rating) >= 600 THEN '플래티넘'
                        WHEN LEAST(1000, user_mastery.rating + EXCLUDED.rating) >= 400 THEN '골드'
                        WHEN LEAST(1000, user_mastery.rating + EXCLUDED.rating) >= 200 THEN '실버'
                        WHEN LEAST(1000, user_mastery.rating + EXCLUDED.rating) >= 50 THEN '브론즈'
                        ELSE '새싹' END,
            evidence_count = user_mastery.evidence_count + 1,
            updated_at = now()
        "#,
    )
    .bind(reward.user_id)
    .bind(reward.axis)
    .bind(reward.mastery_points)
    .execute(&mut **transaction)
    .await?;

    if independent {
        sqlx::query(
            r#"
            INSERT INTO user_achievements (user_id, achievement_slug, evidence_event_id)
            VALUES ($1, 'first-independent', $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(reward.user_id)
        .bind(reward.event_id)
        .execute(&mut **transaction)
        .await?;
        let independent_count: i32 = sqlx::query_scalar(
            "SELECT independent_mastered_count FROM progression_state WHERE user_id = $1",
        )
        .bind(reward.user_id)
        .fetch_one(&mut **transaction)
        .await?;
        if independent_count >= 3 {
            sqlx::query(
                r#"
                INSERT INTO user_achievements (user_id, achievement_slug, evidence_event_id)
                VALUES ($1, 'three-independent', $2) ON CONFLICT DO NOTHING
                "#,
            )
            .bind(reward.user_id)
            .bind(reward.event_id)
            .execute(&mut **transaction)
            .await?;
            sqlx::query(
                r#"
                INSERT INTO user_collectibles (user_id, collectible_kind, collectible_slug) VALUES
                    ($1, 'title', 'independent-solver'),
                    ($1, 'cosmetic', 'independent-aurora')
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(reward.user_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    if current_streak >= 7 {
        sqlx::query(
            r#"
            INSERT INTO user_achievements (user_id, achievement_slug, evidence_event_id)
            VALUES ($1, 'seven-day-streak', $2) ON CONFLICT DO NOTHING
            "#,
        )
        .bind(reward.user_id)
        .bind(reward.event_id)
        .execute(&mut **transaction)
        .await?;
    }
    if reward.axis == "documentation" {
        sqlx::query(
            r#"
            INSERT INTO user_achievements (user_id, achievement_slug, evidence_event_id)
            VALUES ($1, 'docs-reader', $2) ON CONFLICT DO NOTHING
            "#,
        )
        .bind(reward.user_id)
        .bind(reward.event_id)
        .execute(&mut **transaction)
        .await?;
    }

    let quests = sqlx::query_as::<_, QuestRow>(
        r#"
        SELECT id, slug, cadence, target, reward_xp
        FROM quest_definitions
        WHERE active = true AND (metric = 'meaningful_completion' OR (metric = 'independent_mastery' AND $1))
        "#,
    )
    .bind(independent)
    .fetch_all(&mut **transaction)
    .await?;
    let mut total_xp = reward.xp;
    for quest in quests {
        let period_start: Date = sqlx::query_scalar(if quest.cadence == "daily" {
            "SELECT (now() AT TIME ZONE 'Asia/Seoul')::date"
        } else {
            "SELECT ((now() AT TIME ZONE 'Asia/Seoul')::date - (EXTRACT(ISODOW FROM now() AT TIME ZONE 'Asia/Seoul')::integer - 1))"
        })
        .fetch_one(&mut **transaction)
        .await?;
        let progress: i32 = sqlx::query_scalar(
            r#"
            INSERT INTO user_quest_progress (user_id, quest_id, period_start, progress, completed_at)
            VALUES ($1, $2, $3, 1, CASE WHEN $4 <= 1 THEN now() END)
            ON CONFLICT (user_id, quest_id, period_start) DO UPDATE
            SET progress = LEAST($4, user_quest_progress.progress + 1),
                completed_at = CASE WHEN user_quest_progress.progress + 1 >= $4
                                    THEN COALESCE(user_quest_progress.completed_at, now())
                                    ELSE NULL END
            RETURNING progress
            "#,
        )
        .bind(reward.user_id)
        .bind(quest.id)
        .bind(period_start)
        .bind(quest.target)
        .fetch_one(&mut **transaction)
        .await?;
        if progress >= quest.target {
            let quest_event_id: Option<Uuid> = sqlx::query_scalar(
                r#"
                INSERT INTO xp_events (
                    event_id, user_id, reward_key, source_kind, source_id, xp, reason_ko, season_id
                ) VALUES (
                    gen_random_uuid(), $1, $2, 'quest', $3, $4, '퀘스트 완료',
                    (SELECT id FROM seasons WHERE status = 'active' AND starts_at <= now() AND ends_at > now() ORDER BY starts_at DESC LIMIT 1)
                ) ON CONFLICT DO NOTHING RETURNING event_id
                "#,
            )
            .bind(reward.user_id)
            .bind(format!("quest:{}:{}", quest.slug, period_start))
            .bind(quest.id)
            .bind(quest.reward_xp)
            .fetch_optional(&mut **transaction)
            .await?;
            if let Some(quest_event_id) = quest_event_id {
                sqlx::query(
                    "UPDATE user_quest_progress SET reward_event_id = $4 WHERE user_id = $1 AND quest_id = $2 AND period_start = $3",
                )
                .bind(reward.user_id)
                .bind(quest.id)
                .bind(period_start)
                .bind(quest_event_id)
                .execute(&mut **transaction)
                .await?;
                sqlx::query(
                    "UPDATE progression_state SET xp = xp + $2, level = 1 + ((xp + $2) / 500)::integer, updated_at = now() WHERE user_id = $1",
                )
                .bind(reward.user_id)
                .bind(quest.reward_xp)
                .execute(&mut **transaction)
                .await?;
                total_xp += quest.reward_xp;
            }
        }
    }

    Ok(RewardOutcome {
        awarded: true,
        xp_awarded: total_xp,
    })
}

#[derive(Debug)]
pub enum GamificationError {
    Auth(AuthError),
    InvalidInput(&'static str),
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for GamificationError {
    fn into_response(self) -> Response {
        match self {
            Self::Auth(error) => error.into_response(),
            Self::InvalidInput(message) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"code": "invalid_input", "message": message}})),
            )
                .into_response(),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"code": "profile_not_found", "message": "프로필을 찾을 수 없습니다"}})),
            )
                .into_response(),
            Self::Database(error) => {
                tracing::error!(%error, "성취 데이터베이스 처리 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"code": "internal_error", "message": "요청을 처리하지 못했습니다"}})),
                )
                    .into_response()
            }
        }
    }
}

impl From<AuthError> for GamificationError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for GamificationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug, Serialize, FromRow)]
pub struct ProgressionView {
    xp: i64,
    level: i32,
    current_streak: i32,
    best_streak: i32,
    streak_protections: i32,
    mastered_count: i32,
    independent_mastered_count: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct MasteryView {
    axis: String,
    rating: i32,
    tier: String,
    evidence_count: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct QuestView {
    slug: String,
    title: String,
    cadence: String,
    target: i32,
    reward_xp: i32,
    progress: i32,
    completed: bool,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AchievementView {
    slug: String,
    title: String,
    description: String,
    icon_token: String,
    earned_at: OffsetDateTime,
}

#[derive(Debug, Serialize, FromRow)]
pub struct XpEventView {
    xp: i32,
    reason: String,
    source_kind: String,
    occurred_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct DashboardResponse {
    handle: String,
    display_name: String,
    progression: ProgressionView,
    independent_mastery_ratio: f64,
    mastery: Vec<MasteryView>,
    quests: Vec<QuestView>,
    achievements: Vec<AchievementView>,
    recent_xp: Vec<XpEventView>,
    profile: ProfileSettingsView,
    titles: Vec<CollectibleView>,
    cosmetics: Vec<CollectibleView>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ProfileSettingsView {
    bio: String,
    visibility: String,
    show_in_rankings: bool,
    allow_friend_requests: bool,
    selected_title_slug: String,
    selected_cosmetic_slug: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct CollectibleView {
    slug: String,
    label: String,
}

pub async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, GamificationError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let (handle, display_name): (String, String) = sqlx::query_as(
        "SELECT handle, display_name FROM users WHERE id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(GamificationError::NotFound)?;
    let progression = sqlx::query_as::<_, ProgressionView>(
        r#"
        SELECT xp, level, current_streak, best_streak, streak_protections,
               mastered_count, independent_mastered_count
        FROM progression_state WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    let mastery = sqlx::query_as::<_, MasteryView>(
        "SELECT axis, rating, tier, evidence_count FROM user_mastery WHERE user_id = $1 ORDER BY axis",
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let quests = sqlx::query_as::<_, QuestView>(
        r#"
        SELECT quest.slug, quest.title_ko AS title, quest.cadence, quest.target, quest.reward_xp,
               COALESCE(progress.progress, 0) AS progress,
               progress.completed_at IS NOT NULL AS completed
        FROM quest_definitions quest
        LEFT JOIN user_quest_progress progress
          ON progress.quest_id = quest.id AND progress.user_id = $1
         AND progress.period_start = CASE WHEN quest.cadence = 'daily'
             THEN (now() AT TIME ZONE 'Asia/Seoul')::date
             ELSE ((now() AT TIME ZONE 'Asia/Seoul')::date - (EXTRACT(ISODOW FROM now() AT TIME ZONE 'Asia/Seoul')::integer - 1)) END
        WHERE quest.active = true ORDER BY quest.cadence, quest.slug
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let achievements = sqlx::query_as::<_, AchievementView>(
        r#"
        SELECT definition.slug, definition.title_ko AS title,
               definition.description_ko AS description, definition.icon_token,
               earned.earned_at
        FROM user_achievements earned
        JOIN achievement_definitions definition ON definition.slug = earned.achievement_slug
        WHERE earned.user_id = $1 ORDER BY earned.earned_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let recent_xp = sqlx::query_as::<_, XpEventView>(
        r#"
        SELECT xp, reason_ko AS reason, source_kind, occurred_at
        FROM xp_events WHERE user_id = $1 ORDER BY occurred_at DESC LIMIT 10
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let profile = sqlx::query_as::<_, ProfileSettingsView>(
        r#"
        SELECT bio_ko AS bio, visibility, show_in_rankings, allow_friend_requests,
               selected_title_slug, selected_cosmetic_slug
        FROM profile_preferences WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;
    let titles = sqlx::query_as::<_, CollectibleView>(
        r#"
        SELECT definition.slug, definition.label_ko AS label
        FROM user_collectibles owned
        JOIN title_definitions definition ON definition.slug = owned.collectible_slug
        WHERE owned.user_id = $1 AND owned.collectible_kind = 'title'
        ORDER BY definition.label_ko
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let cosmetics = sqlx::query_as::<_, CollectibleView>(
        r#"
        SELECT definition.slug, definition.label_ko AS label
        FROM user_collectibles owned
        JOIN cosmetic_definitions definition ON definition.slug = owned.collectible_slug
        WHERE owned.user_id = $1 AND owned.collectible_kind = 'cosmetic'
        ORDER BY definition.label_ko
        "#,
    )
    .bind(user_id)
    .fetch_all(state.pool())
    .await?;
    let ratio = if progression.mastered_count == 0 {
        0.0
    } else {
        f64::from(progression.independent_mastered_count) / f64::from(progression.mastered_count)
    };
    Ok(Json(DashboardResponse {
        handle,
        display_name,
        progression,
        independent_mastery_ratio: ratio,
        mastery,
        quests,
        achievements,
        recent_xp,
        profile,
        titles,
        cosmetics,
    }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct PublicProfileResponse {
    handle: String,
    display_name: String,
    bio: String,
    title: Option<String>,
    cosmetic: Option<String>,
    level: i32,
    current_streak: i32,
    independent_mastery_ratio: f64,
    achievements: i64,
}

pub async fn profile(
    State(state): State<AppState>,
    Path(handle): Path<String>,
) -> Result<Json<PublicProfileResponse>, GamificationError> {
    let profile = sqlx::query_as::<_, PublicProfileResponse>(
        r#"
        SELECT user_account.handle, user_account.display_name, preferences.bio_ko AS bio,
               title.label_ko AS title, cosmetic.css_token AS cosmetic,
               progression.level, progression.current_streak,
               CASE WHEN progression.mastered_count = 0 THEN 0::float8
                    ELSE progression.independent_mastered_count::float8 / progression.mastered_count END AS independent_mastery_ratio,
               (SELECT COUNT(*) FROM user_achievements WHERE user_id = user_account.id) AS achievements
        FROM users user_account
        JOIN profile_preferences preferences ON preferences.user_id = user_account.id
        JOIN progression_state progression ON progression.user_id = user_account.id
        LEFT JOIN title_definitions title ON title.slug = preferences.selected_title_slug
        LEFT JOIN cosmetic_definitions cosmetic ON cosmetic.slug = preferences.selected_cosmetic_slug
        WHERE user_account.handle = $1 AND user_account.status = 'active'
          AND preferences.visibility = 'public'
        "#,
    )
    .bind(handle)
    .fetch_optional(state.pool())
    .await?
    .ok_or(GamificationError::NotFound)?;
    Ok(Json(profile))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileRequest {
    display_name: String,
    bio: String,
    visibility: String,
    show_in_rankings: bool,
    allow_friend_requests: bool,
    selected_title_slug: String,
    selected_cosmetic_slug: String,
}

pub async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<StatusCode, GamificationError> {
    let user_id = crate::auth::authenticated_user_id_with_csrf(&state, &headers).await?;
    if !(1..=80).contains(&request.display_name.chars().count())
        || request.bio.chars().count() > 500
        || !matches!(request.visibility.as_str(), "public" | "private")
    {
        return Err(GamificationError::InvalidInput(
            "프로필 이름, 소개, 공개 범위를 확인해 주세요",
        ));
    }
    let owned: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM user_collectibles WHERE user_id = $1 AND collectible_kind = 'title' AND collectible_slug = $2
        ) AND EXISTS (
            SELECT 1 FROM user_collectibles WHERE user_id = $1 AND collectible_kind = 'cosmetic' AND collectible_slug = $3
        )
        "#,
    )
    .bind(user_id)
    .bind(&request.selected_title_slug)
    .bind(&request.selected_cosmetic_slug)
    .fetch_one(state.pool())
    .await?;
    if !owned {
        return Err(GamificationError::InvalidInput(
            "보유한 칭호와 꾸미기만 선택할 수 있습니다",
        ));
    }
    let mut transaction = state.pool().begin().await?;
    sqlx::query("UPDATE users SET display_name = $2, updated_at = now() WHERE id = $1")
        .bind(user_id)
        .bind(request.display_name.trim())
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"
        UPDATE profile_preferences
        SET bio_ko = $2, visibility = $3, show_in_rankings = $4,
            allow_friend_requests = $5, selected_title_slug = $6,
            selected_cosmetic_slug = $7, updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .bind(request.bio.trim())
    .bind(request.visibility)
    .bind(request.show_in_rankings)
    .bind(request.allow_friend_requests)
    .bind(request.selected_title_slug)
    .bind(request.selected_cosmetic_slug)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankingQuery {
    axis: Option<String>,
    season: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct RankingEntry {
    rank: i64,
    handle: String,
    display_name: String,
    score: i64,
    tier: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RankingResponse {
    scope: String,
    entries: Vec<RankingEntry>,
    privacy_note: &'static str,
}

pub async fn rankings(
    State(state): State<AppState>,
    Query(query): Query<RankingQuery>,
) -> Result<Json<RankingResponse>, GamificationError> {
    let valid_axis = query.axis.as_deref().is_some_and(|axis| {
        matches!(
            axis,
            "algorithm"
                | "code_reading"
                | "debugging"
                | "documentation"
                | "framework"
                | "contest"
                | "instructor_course"
        )
    });
    if query.axis.is_some() && !valid_axis {
        return Err(GamificationError::InvalidInput(
            "지원하지 않는 랭킹 축입니다",
        ));
    }
    let (scope, entries) = if let Some(axis) = query.axis {
        let rows = sqlx::query_as::<_, RankingEntry>(
            r#"
            SELECT ROW_NUMBER() OVER (ORDER BY mastery.rating DESC, mastery.updated_at, user_account.id)::bigint AS rank,
                   user_account.handle, user_account.display_name,
                   mastery.rating::bigint AS score, mastery.tier,
                   title.label_ko AS title
            FROM user_mastery mastery
            JOIN users user_account ON user_account.id = mastery.user_id
            JOIN profile_preferences preferences ON preferences.user_id = user_account.id
            LEFT JOIN title_definitions title ON title.slug = preferences.selected_title_slug
            WHERE mastery.axis = $1 AND preferences.visibility = 'public'
              AND preferences.show_in_rankings = true AND user_account.status = 'active'
            ORDER BY mastery.rating DESC, mastery.updated_at, user_account.id LIMIT 100
            "#,
        )
        .bind(&axis)
        .fetch_all(state.pool())
        .await?;
        (axis, rows)
    } else if let Some(season) = query.season {
        let rows = sqlx::query_as::<_, RankingEntry>(
            r#"
            SELECT ROW_NUMBER() OVER (ORDER BY SUM(event.xp) DESC, user_account.id)::bigint AS rank,
                   user_account.handle, user_account.display_name,
                   SUM(event.xp)::bigint AS score, NULL::text AS tier,
                   title.label_ko AS title
            FROM xp_events event
            JOIN seasons season ON season.id = event.season_id
            JOIN users user_account ON user_account.id = event.user_id
            JOIN profile_preferences preferences ON preferences.user_id = user_account.id
            LEFT JOIN title_definitions title ON title.slug = preferences.selected_title_slug
            WHERE season.slug = $1 AND preferences.visibility = 'public'
              AND preferences.show_in_rankings = true AND user_account.status = 'active'
            GROUP BY user_account.id, user_account.handle, user_account.display_name, title.label_ko
            ORDER BY score DESC, user_account.id LIMIT 100
            "#,
        )
        .bind(&season)
        .fetch_all(state.pool())
        .await?;
        (format!("season:{season}"), rows)
    } else {
        let rows = sqlx::query_as::<_, RankingEntry>(
            r#"
            SELECT ROW_NUMBER() OVER (ORDER BY progression.xp DESC, progression.updated_at, user_account.id)::bigint AS rank,
                   user_account.handle, user_account.display_name,
                   progression.xp AS score, NULL::text AS tier,
                   title.label_ko AS title
            FROM progression_state progression
            JOIN users user_account ON user_account.id = progression.user_id
            JOIN profile_preferences preferences ON preferences.user_id = user_account.id
            LEFT JOIN title_definitions title ON title.slug = preferences.selected_title_slug
            WHERE preferences.visibility = 'public' AND preferences.show_in_rankings = true
              AND user_account.status = 'active'
            ORDER BY progression.xp DESC, progression.updated_at, user_account.id LIMIT 100
            "#,
        )
        .fetch_all(state.pool())
        .await?;
        ("overall".to_owned(), rows)
    };
    Ok(Json(RankingResponse {
        scope,
        entries,
        privacy_note: "비공개 또는 랭킹 제외 프로필은 표시하지 않습니다.",
    }))
}
