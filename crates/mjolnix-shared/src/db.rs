use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgListener, PgPoolOptions, PgRow};
use sqlx::types::Json;
use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::store;

pub type DbPool = PgPool;

/// `LISTEN` channel; workers wake when new builds are queued or requeued.
pub const BUILD_QUEUED_CHANNEL: &str = "mjolnix_build_queued";

pub const BUILD_HEARTBEAT_INTERVAL_SECS: u64 = 10;
pub const BUILD_HEARTBEAT_STALE_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
}

impl BuildStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "success" => Self::Success,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub id: i64,
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Build {
    pub id: i64,
    pub repo_id: i64,
    pub rev: String,
    pub ref_name: String,
    pub status: BuildStatus,
    pub flake_attr: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub log_path: Option<String>,
    pub error_summary: Option<String>,
    pub closure_paths: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub last_heartbeat: Option<DateTime<Utc>>,
}

pub async fn connect(config: &Config) -> Result<DbPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("connect to PostgreSQL (check MJOLNIX_DATABASE_URL)")
}

pub async fn migrate(pool: &DbPool) -> Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

pub async fn connect_listener(database_url: &str) -> Result<PgListener> {
    PgListener::connect(database_url)
        .await
        .context("connect for LISTEN")
}

pub async fn create_user(pool: &DbPool) -> Result<i64> {
    sqlx::query_scalar("INSERT INTO users DEFAULT VALUES RETURNING id")
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn user_id_for_fingerprint(pool: &DbPool, fingerprint: &str) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT user_id FROM ssh_keys WHERE fingerprint = $1")
        .bind(fingerprint)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("user_id")))
}

pub async fn attach_key(pool: &DbPool, fingerprint: &str, user_id: i64) -> Result<()> {
    sqlx::query("INSERT INTO ssh_keys (fingerprint, user_id) VALUES ($1, $2)")
        .bind(fingerprint)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_or_create_dev_user(pool: &DbPool) -> Result<i64> {
    const DEV_FINGERPRINT: &str = "dev:local";
    if let Some(id) = user_id_for_fingerprint(pool, DEV_FINGERPRINT).await? {
        return Ok(id);
    }
    let user_id = create_user(pool).await?;
    attach_key(pool, DEV_FINGERPRINT, user_id).await?;
    Ok(user_id)
}

pub async fn list_repos_for_user(pool: &DbPool, user_id: i64) -> Result<Vec<Repo>> {
    let rows = sqlx::query(
        "SELECT id, namespace, name FROM repos WHERE user_id = $1 ORDER BY namespace, name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_repo).collect())
}

pub async fn get_repo(pool: &DbPool, namespace: &str, name: &str) -> Result<Option<Repo>> {
    let row =
        sqlx::query("SELECT id, namespace, name FROM repos WHERE namespace = $1 AND name = $2")
            .bind(namespace)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(row_to_repo))
}

pub async fn get_repo_by_id(pool: &DbPool, repo_id: i64) -> Result<Option<Repo>> {
    let row = sqlx::query("SELECT id, namespace, name FROM repos WHERE id = $1")
        .bind(repo_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(row_to_repo))
}

pub async fn create_repo(
    pool: &DbPool,
    config: &Config,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<i64> {
    let repo_id: i64 = sqlx::query_scalar(
        "INSERT INTO repos (user_id, namespace, name) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(namespace)
    .bind(name)
    .fetch_one(pool)
    .await?;
    let store_root = store::store_root_for_repo(config, repo_id);
    store::ensure_store_root(&store_root).await?;
    Ok(repo_id)
}

pub async fn user_owns_repo(
    pool: &DbPool,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<bool> {
    let row = sqlx::query(
        "SELECT 1 AS ok FROM repos WHERE user_id = $1 AND namespace = $2 AND name = $3",
    )
    .bind(user_id)
    .bind(namespace)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub async fn notify_build_queued(pool: &DbPool) -> Result<()> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(BUILD_QUEUED_CHANNEL)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_build_queued(
    pool: &DbPool,
    repo_id: i64,
    rev: &str,
    ref_name: &str,
) -> Result<i64> {
    let id = sqlx::query_scalar(
        "INSERT INTO builds (repo_id, rev, ref_name, status) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(repo_id)
    .bind(rev)
    .bind(ref_name)
    .bind(BuildStatus::Queued.as_str())
    .fetch_one(pool)
    .await?;
    notify_build_queued(pool).await?;
    Ok(id)
}

pub async fn set_build_running(pool: &DbPool, build_id: i64, log_path: &str) -> Result<()> {
    sqlx::query(
        "UPDATE builds SET status = $1, started_at = NOW(), log_path = $2, last_heartbeat = NOW() WHERE id = $3 AND status = $4",
    )
    .bind(BuildStatus::Running.as_str())
    .bind(log_path)
    .bind(build_id)
    .bind(BuildStatus::Running.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_build_success(
    pool: &DbPool,
    build_id: i64,
    closure_paths: &[String],
) -> Result<()> {
    let paths = Json(closure_paths.to_vec());
    sqlx::query(
        "UPDATE builds SET status = $1, finished_at = NOW(), closure_paths = $2, error_summary = NULL WHERE id = $3 AND status = $4",
    )
    .bind(BuildStatus::Success.as_str())
    .bind(paths)
    .bind(build_id)
    .bind(BuildStatus::Running.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_build_failed(pool: &DbPool, build_id: i64, error_summary: &str) -> Result<()> {
    sqlx::query(
        "UPDATE builds SET status = $1, finished_at = NOW(), error_summary = $2 WHERE id = $3 AND status = $4",
    )
    .bind(BuildStatus::Failed.as_str())
    .bind(error_summary)
    .bind(build_id)
    .bind(BuildStatus::Running.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_build(pool: &DbPool, build_id: i64) -> Result<Option<Build>> {
    let row = sqlx::query("SELECT * FROM builds WHERE id = $1")
        .bind(build_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| row_to_build(&r)))
}

pub async fn list_builds_for_repo(pool: &DbPool, repo_id: i64, limit: i64) -> Result<Vec<Build>> {
    let rows =
        sqlx::query("SELECT * FROM builds WHERE repo_id = $1 ORDER BY created_at DESC LIMIT $2")
            .bind(repo_id)
            .bind(limit)
            .fetch_all(pool)
            .await?;
    Ok(rows.iter().map(row_to_build).collect())
}

pub async fn latest_build_for_repo(pool: &DbPool, repo_id: i64) -> Result<Option<Build>> {
    let row =
        sqlx::query("SELECT * FROM builds WHERE repo_id = $1 ORDER BY created_at DESC LIMIT 1")
            .bind(repo_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| row_to_build(&r)))
}

/// Atomically claim the oldest queued build for this worker (safe across multiple workers).
pub async fn claim_next_queued_build(pool: &DbPool) -> Result<Option<Build>> {
    let row = sqlx::query(
        r#"
        UPDATE builds
        SET status = $1, started_at = NOW(), last_heartbeat = NOW()
        WHERE id = (
            SELECT id FROM builds
            WHERE status = $2
            ORDER BY created_at
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        RETURNING *
        "#,
    )
    .bind(BuildStatus::Running.as_str())
    .bind(BuildStatus::Queued.as_str())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| row_to_build(&r)))
}

pub async fn touch_build_heartbeat(pool: &DbPool, build_id: i64) -> Result<()> {
    sqlx::query("UPDATE builds SET last_heartbeat = NOW() WHERE id = $1 AND status = $2")
        .bind(build_id)
        .bind(BuildStatus::Running.as_str())
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark running builds whose heartbeat is older than [`BUILD_HEARTBEAT_STALE_SECS`] as failed.
pub async fn fail_stale_running_builds(pool: &DbPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE builds
        SET status = $1, finished_at = NOW(), error_summary = $2
        WHERE status = $3
          AND last_heartbeat IS NOT NULL
          AND last_heartbeat < NOW() - ($4::bigint * INTERVAL '1 second')
        "#,
    )
    .bind(BuildStatus::Failed.as_str())
    .bind("worker heartbeat timeout")
    .bind(BuildStatus::Running.as_str())
    .bind(BUILD_HEARTBEAT_STALE_SECS as i64)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

fn row_to_repo(r: PgRow) -> Repo {
    Repo {
        id: r.get("id"),
        namespace: r.get("namespace"),
        name: r.get("name"),
    }
}

fn row_to_build(r: &PgRow) -> Build {
    let status: String = r.get("status");
    let closure_paths: Option<Json<Vec<String>>> = r.get("closure_paths");
    Build {
        id: r.get("id"),
        repo_id: r.get("repo_id"),
        rev: r.get("rev"),
        ref_name: r.get("ref_name"),
        status: BuildStatus::parse(&status).unwrap_or(BuildStatus::Failed),
        flake_attr: r.get("flake_attr"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        log_path: r.get("log_path"),
        error_summary: r.get("error_summary"),
        closure_paths: closure_paths.map(|j| j.0),
        created_at: r.get("created_at"),
        last_heartbeat: r.get("last_heartbeat"),
    }
}
