use alpha::{
    gamification::{RewardSpec, apply_reward},
    http::{AppState, router},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn user(pool: &PgPool, handle: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (handle, display_name) VALUES ($1, $2) RETURNING id")
        .bind(handle)
        .bind(handle)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn reward(user_id: Uuid, event_id: Uuid, reward_key: String) -> RewardSpec {
    RewardSpec {
        event_id,
        user_id,
        reward_key,
        source_kind: "activity",
        source_id: Some(Uuid::now_v7()),
        xp: 50,
        reason_ko: "독립 활동 통과",
        axis: "code_reading",
        mastery_points: 20,
        mastery_class: "independent",
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn reward_ledger_blocks_replay_and_farming_while_quests_unlock_once(pool: PgPool) {
    let user_id = user(&pool, "reward-learner").await;
    let first_event = Uuid::now_v7();
    let mut transaction = pool.begin().await.unwrap();
    let first = apply_reward(
        &mut transaction,
        reward(user_id, first_event, "activity:first".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(
        first.xp_awarded, 70,
        "첫 의미 있는 활동은 일일 퀘스트를 함께 완료한다"
    );
    assert!(
        !apply_reward(
            &mut transaction,
            reward(user_id, first_event, "activity:first".to_owned()),
        )
        .await
        .unwrap()
        .awarded
    );
    assert!(
        !apply_reward(
            &mut transaction,
            reward(user_id, Uuid::now_v7(), "activity:first".to_owned()),
        )
        .await
        .unwrap()
        .awarded,
        "새 event_id로 같은 활동을 반복해도 보상할 수 없어야 한다"
    );
    apply_reward(
        &mut transaction,
        reward(user_id, Uuid::now_v7(), "activity:second".to_owned()),
    )
    .await
    .unwrap();
    let third = apply_reward(
        &mut transaction,
        reward(user_id, Uuid::now_v7(), "activity:third".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(
        third.xp_awarded, 130,
        "세 번째 독립 해결은 주간 퀘스트를 한 번 완료한다"
    );
    transaction.commit().await.unwrap();

    let state: (i64, i32, i32) = sqlx::query_as(
        "SELECT xp, mastered_count, independent_mastered_count FROM progression_state WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, (250, 3, 3));
    let ledger_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM xp_events WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ledger_events, 5,
        "활동 3개와 일일·주간 퀘스트만 기록돼야 한다"
    );
    let unlocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_achievements WHERE user_id = $1 AND achievement_slug = 'three-independent')",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(unlocked);
}

#[sqlx::test(migrations = "./migrations")]
async fn private_and_opted_out_profiles_never_enter_public_rankings(pool: PgPool) {
    let public_id = user(&pool, "public-learner").await;
    let private_id = user(&pool, "private-learner").await;
    let opted_out_id = user(&pool, "hidden-rank").await;
    sqlx::query("UPDATE progression_state SET xp = CASE user_id WHEN $1 THEN 100 WHEN $2 THEN 999 ELSE 800 END")
        .bind(public_id)
        .bind(private_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE profile_preferences SET visibility = 'private' WHERE user_id = $1")
        .bind(private_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE profile_preferences SET show_in_rankings = false WHERE user_id = $1")
        .bind(opted_out_id)
        .execute(&pool)
        .await
        .unwrap();

    let app = router(AppState::for_test(pool));
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/rankings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32_768).await.unwrap()).unwrap();
    let handles: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["handle"].as_str())
        .collect();
    assert_eq!(handles, vec!["public-learner"]);

    let private_profile = app
        .oneshot(
            Request::get("/api/v1/profiles/private-learner")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(private_profile.status(), StatusCode::NOT_FOUND);
}
