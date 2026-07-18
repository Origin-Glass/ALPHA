use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Accepted,
    WrongAnswer,
    PartialAccepted,
    TimeLimitExceeded,
    MemoryLimitExceeded,
    OutputLimitExceeded,
    RuntimeError,
    CompileError,
    SystemError,
    Cancelled,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::WrongAnswer => "WRONG_ANSWER",
            Self::PartialAccepted => "PARTIAL_ACCEPTED",
            Self::TimeLimitExceeded => "TIME_LIMIT_EXCEEDED",
            Self::MemoryLimitExceeded => "MEMORY_LIMIT_EXCEEDED",
            Self::OutputLimitExceeded => "OUTPUT_LIMIT_EXCEEDED",
            Self::RuntimeError => "RUNTIME_ERROR",
            Self::CompileError => "COMPILE_ERROR",
            Self::SystemError => "SYSTEM_ERROR",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Checker {
    Exact,
    Whitespace,
    Float { tolerance: f64 },
}

impl Checker {
    pub fn accepts(self, actual: &str, expected: &str) -> bool {
        match self {
            Self::Exact => actual == expected,
            Self::Whitespace => actual.split_whitespace().eq(expected.split_whitespace()),
            Self::Float { tolerance } => {
                let actual: Result<Vec<f64>, _> =
                    actual.split_whitespace().map(str::parse).collect();
                let expected: Result<Vec<f64>, _> =
                    expected.split_whitespace().map(str::parse).collect();
                match (actual, expected) {
                    (Ok(actual), Ok(expected)) if actual.len() == expected.len() => actual
                        .iter()
                        .zip(expected.iter())
                        .all(|(actual, expected)| {
                            actual.is_finite()
                                && expected.is_finite()
                                && (actual - expected).abs() <= tolerance * expected.abs().max(1.0)
                        }),
                    _ => false,
                }
            }
        }
    }
}

#[derive(Debug, FromRow)]
pub struct LeasedJob {
    pub job_id: Uuid,
    pub submission_id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i16,
    pub language: String,
    pub source: String,
    pub problem_id: Uuid,
    pub problem_revision_id: Uuid,
    pub time_limit_ms: i32,
    pub memory_limit_mb: i32,
    pub checker_kind: String,
    pub float_tolerance: Option<f64>,
    pub run_kind: String,
    pub custom_input: Option<String>,
}

#[derive(Debug, FromRow)]
pub struct JudgeTestCase {
    pub ordinal: i32,
    pub input: String,
    pub expected_output: String,
    pub score_weight: i32,
    pub group_key: String,
}

#[derive(Debug)]
pub enum QueueError {
    LeaseLost,
    InvalidVerdict,
    Database(sqlx::Error),
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeaseLost => formatter.write_str("판정 작업 임대가 만료됐습니다"),
            Self::InvalidVerdict => formatter.write_str("판정 결과가 올바르지 않습니다"),
            Self::Database(error) => write!(formatter, "판정 큐 데이터베이스 오류: {error}"),
        }
    }
}

impl std::error::Error for QueueError {}

impl From<sqlx::Error> for QueueError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub async fn lease_next_job(
    pool: &PgPool,
    worker_id: &str,
    visibility_timeout: Duration,
) -> Result<Option<LeasedJob>, QueueError> {
    let mut transaction = pool.begin().await?;
    let dead_submissions: Vec<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE judge_jobs
        SET status = 'dead', lease_owner = NULL, lease_token = NULL,
            lease_expires_at = NULL, updated_at = now(), last_error = '최대 재시도 횟수 초과'
        WHERE status = 'leased' AND lease_expires_at <= now() AND attempt_count >= max_attempts
        RETURNING submission_id
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    for submission_id in dead_submissions {
        sqlx::query(
            "UPDATE submissions SET status = 'SYSTEM_ERROR', score = 0, judged_at = now() WHERE id = $1 AND judged_at IS NULL",
        )
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO submission_events (submission_id, status, safe_message) VALUES ($1, 'SYSTEM_ERROR', '판정 작업 재시도 한도를 초과했습니다')",
        )
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        UPDATE judge_jobs
        SET status = 'ready', lease_owner = NULL, lease_token = NULL,
            lease_expires_at = NULL, available_at = now(), updated_at = now(),
            last_error = '이전 작업자 임대 만료'
        WHERE status = 'leased' AND lease_expires_at <= now() AND attempt_count < max_attempts
        "#,
    )
    .execute(&mut *transaction)
    .await?;

    let lease_token = Uuid::now_v7();
    let lease_seconds = i64::try_from(visibility_timeout.as_secs().clamp(5, 3600))
        .expect("제한된 초 값은 i64에 들어간다");
    let lease_expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(lease_seconds);
    let leased: Option<(Uuid, Uuid, i16)> = sqlx::query_as(
        r#"
        WITH selected AS (
            SELECT id FROM judge_jobs
            WHERE status = 'ready' AND available_at <= now() AND attempt_count < max_attempts
            ORDER BY available_at, created_at
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE judge_jobs job
        SET status = 'leased', attempt_count = attempt_count + 1,
            lease_owner = $1, lease_token = $2, lease_expires_at = $3, updated_at = now()
        FROM selected
        WHERE job.id = selected.id
        RETURNING job.id, job.submission_id, job.attempt_count
        "#,
    )
    .bind(worker_id)
    .bind(lease_token)
    .bind(lease_expires_at)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((job_id, submission_id, _attempt)) = leased else {
        transaction.commit().await?;
        return Ok(None);
    };
    sqlx::query("UPDATE submissions SET status = 'COMPILING' WHERE id = $1 AND judged_at IS NULL")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO submission_events (submission_id, status) VALUES ($1, 'COMPILING')")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    let job = sqlx::query_as::<_, LeasedJob>(
        r#"
        SELECT job.id AS job_id, submission.id AS submission_id, job.lease_token,
               job.attempt_count AS attempt, submission.language, submission.source,
               submission.problem_id, submission.problem_revision_id,
               revision.time_limit_ms, revision.memory_limit_mb,
               revision.checker_kind, revision.float_tolerance,
               submission.run_kind, submission.custom_input
        FROM judge_jobs job
        JOIN submissions submission ON submission.id = job.submission_id
        JOIN problem_revisions revision ON revision.id = submission.problem_revision_id
        WHERE job.id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(job))
}

pub async fn load_test_cases(
    pool: &PgPool,
    problem_id: Uuid,
    run_kind: &str,
) -> Result<Vec<JudgeTestCase>, QueueError> {
    Ok(sqlx::query_as::<_, JudgeTestCase>(
        r#"
        SELECT ordinal, input, expected_output, score_weight, group_key
        FROM problem_test_cases
        WHERE problem_id = $1
          AND ($2 <> 'sample' OR visibility = 'sample')
        ORDER BY ordinal
        "#,
    )
    .bind(problem_id)
    .bind(run_kind)
    .fetch_all(pool)
    .await?)
}

pub async fn record_running(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
) -> Result<(), QueueError> {
    let submission_id: Uuid = sqlx::query_scalar(
        "SELECT submission_id FROM judge_jobs WHERE id = $1 AND lease_token = $2 AND status = 'leased' AND lease_expires_at > now()",
    )
    .bind(job_id)
    .bind(lease_token)
    .fetch_optional(pool)
    .await?
    .ok_or(QueueError::LeaseLost)?;
    let mut transaction = pool.begin().await?;
    sqlx::query("UPDATE submissions SET status = 'RUNNING' WHERE id = $1 AND judged_at IS NULL")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO submission_events (submission_id, status) VALUES ($1, 'RUNNING')")
        .bind(submission_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn complete_job(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    verdict: Verdict,
    score: i16,
    compile_output: Option<String>,
) -> Result<(), QueueError> {
    complete_job_with_output(
        pool,
        job_id,
        lease_token,
        verdict,
        score,
        compile_output,
        None,
    )
    .await
}

pub async fn complete_job_with_output(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    verdict: Verdict,
    score: i16,
    compile_output: Option<String>,
    run_output: Option<String>,
) -> Result<(), QueueError> {
    if !(0..=100).contains(&score)
        || (verdict == Verdict::Accepted && score != 100)
        || compile_output
            .as_ref()
            .is_some_and(|output| output.len() > 16_384)
        || run_output
            .as_ref()
            .is_some_and(|output| output.len() > 1_048_576)
    {
        return Err(QueueError::InvalidVerdict);
    }
    let mut transaction = pool.begin().await?;
    let submission_id: Uuid = sqlx::query_scalar(
        r#"
        UPDATE judge_jobs
        SET status = 'done', lease_owner = NULL, lease_token = NULL,
            lease_expires_at = NULL, updated_at = now()
        WHERE id = $1 AND lease_token = $2 AND status = 'leased' AND lease_expires_at > now()
        RETURNING submission_id
        "#,
    )
    .bind(job_id)
    .bind(lease_token)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(QueueError::LeaseLost)?;
    sqlx::query(
        "UPDATE submissions SET status = $2, score = $3, compile_output = $4, run_output = $5, judged_at = now() WHERE id = $1 AND judged_at IS NULL",
    )
    .bind(submission_id)
    .bind(verdict.as_str())
    .bind(score)
    .bind(compile_output)
    .bind(run_output)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO submission_events (submission_id, status) VALUES ($1, $2)")
        .bind(submission_id)
        .bind(verdict.as_str())
        .execute(&mut *transaction)
        .await?;
    if verdict == Verdict::Accepted {
        let reward_source: Option<(Uuid, Uuid, String, String, Option<Uuid>)> = sqlx::query_as(
            r#"
            SELECT submission.user_id, submission.problem_id, problem.learning_axis,
                   submission.run_kind, submission.contest_id
            FROM submissions submission
            JOIN problems problem ON problem.id = submission.problem_id
            WHERE submission.id = $1
            "#,
        )
        .bind(submission_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((user_id, problem_id, learning_axis, run_kind, contest_id)) = reward_source
            && run_kind == "formal"
        {
            let axis = if contest_id.is_some() {
                "contest"
            } else {
                match learning_axis.as_str() {
                    "code_literacy" => "code_reading",
                    "docs_learning" => "documentation",
                    "independent_coding" => "framework",
                    _ => "algorithm",
                }
            };
            let (reward_key, source_kind, source_id, xp, reason, mastery_class) =
                if let Some(contest_id) = contest_id {
                    (
                        format!("contest:{contest_id}:problem:{problem_id}"),
                        "contest",
                        Some(contest_id),
                        100,
                        "대회 문제 해결",
                        "contest_verified",
                    )
                } else {
                    (
                        format!("problem:{problem_id}"),
                        "problem",
                        Some(problem_id),
                        80,
                        "문제 독립 해결",
                        "independent",
                    )
                };
            crate::gamification::apply_reward(
                &mut transaction,
                crate::gamification::RewardSpec {
                    event_id: submission_id,
                    user_id,
                    reward_key,
                    source_kind,
                    source_id,
                    xp,
                    reason_ko: reason,
                    axis,
                    mastery_points: 40,
                    mastery_class,
                },
            )
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}
