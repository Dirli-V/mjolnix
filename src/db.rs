use anyhow::{Context, Result};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::config::Config;

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
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub log_path: Option<String>,
    pub error_summary: Option<String>,
    pub store_paths: Option<Vec<String>>,
    pub created_at: String,
}

pub async fn connect(config: &Config) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);

    SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("connect to {}", config.db_path.display()))
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS ssh_keys (
            fingerprint TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS repos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id),
            namespace TEXT NOT NULL DEFAULT 'public',
            name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(namespace, name)
        );

        CREATE TABLE IF NOT EXISTS builds (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id INTEGER NOT NULL REFERENCES repos(id),
            rev TEXT NOT NULL,
            ref_name TEXT NOT NULL,
            status TEXT NOT NULL,
            flake_attr TEXT,
            started_at TEXT,
            finished_at TEXT,
            log_path TEXT,
            error_summary TEXT,
            store_paths TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_builds_repo_created ON builds(repo_id, created_at DESC);
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn create_user(pool: &SqlitePool) -> Result<i64> {
    let result = sqlx::query("INSERT INTO users DEFAULT VALUES")
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}

pub async fn user_id_for_fingerprint(pool: &SqlitePool, fingerprint: &str) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT user_id FROM ssh_keys WHERE fingerprint = ?")
        .bind(fingerprint)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("user_id")))
}

pub async fn attach_key(pool: &SqlitePool, fingerprint: &str, user_id: i64) -> Result<()> {
    sqlx::query("INSERT INTO ssh_keys (fingerprint, user_id) VALUES (?, ?)")
        .bind(fingerprint)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_or_create_dev_user(pool: &SqlitePool) -> Result<i64> {
    const DEV_FINGERPRINT: &str = "dev:local";

    if let Some(id) = user_id_for_fingerprint(pool, DEV_FINGERPRINT).await? {
        return Ok(id);
    }

    let user_id = create_user(pool).await?;
    attach_key(pool, DEV_FINGERPRINT, user_id).await?;
    Ok(user_id)
}

pub async fn list_repos_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Repo>> {
    let rows = sqlx::query(
        "SELECT id, namespace, name FROM repos WHERE user_id = ? ORDER BY namespace, name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Repo {
            id: r.get("id"),
            namespace: r.get("namespace"),
            name: r.get("name"),
        })
        .collect())
}

pub async fn get_repo(pool: &SqlitePool, namespace: &str, name: &str) -> Result<Option<Repo>> {
    let row = sqlx::query("SELECT id, namespace, name FROM repos WHERE namespace = ? AND name = ?")
        .bind(namespace)
        .bind(name)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| Repo {
        id: r.get("id"),
        namespace: r.get("namespace"),
        name: r.get("name"),
    }))
}

pub async fn create_repo(
    pool: &SqlitePool,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<i64> {
    let result = sqlx::query("INSERT INTO repos (user_id, namespace, name) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(namespace)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}

pub async fn user_owns_repo(
    pool: &SqlitePool,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<bool> {
    let row =
        sqlx::query("SELECT 1 AS ok FROM repos WHERE user_id = ? AND namespace = ? AND name = ?")
            .bind(user_id)
            .bind(namespace)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

pub async fn insert_build_queued(
    pool: &SqlitePool,
    repo_id: i64,
    rev: &str,
    ref_name: &str,
) -> Result<i64> {
    let result =
        sqlx::query("INSERT INTO builds (repo_id, rev, ref_name, status) VALUES (?, ?, ?, ?)")
            .bind(repo_id)
            .bind(rev)
            .bind(ref_name)
            .bind(BuildStatus::Queued.as_str())
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

pub async fn set_build_running(pool: &SqlitePool, build_id: i64, log_path: &str) -> Result<()> {
    sqlx::query(
        "UPDATE builds SET status = ?, started_at = datetime('now'), log_path = ? WHERE id = ?",
    )
    .bind(BuildStatus::Running.as_str())
    .bind(log_path)
    .bind(build_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_build_success(
    pool: &SqlitePool,
    build_id: i64,
    store_paths: &[String],
) -> Result<()> {
    let paths_json = serde_json::to_string(store_paths)?;
    sqlx::query(
        "UPDATE builds SET status = ?, finished_at = datetime('now'), store_paths = ?, error_summary = NULL WHERE id = ?",
    )
    .bind(BuildStatus::Success.as_str())
    .bind(paths_json)
    .bind(build_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_build_failed(pool: &SqlitePool, build_id: i64, error_summary: &str) -> Result<()> {
    sqlx::query(
        "UPDATE builds SET status = ?, finished_at = datetime('now'), error_summary = ? WHERE id = ?",
    )
    .bind(BuildStatus::Failed.as_str())
    .bind(error_summary)
    .bind(build_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_build(pool: &SqlitePool, build_id: i64) -> Result<Option<Build>> {
    let row = sqlx::query("SELECT * FROM builds WHERE id = ?")
        .bind(build_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| row_to_build(&r)))
}

pub async fn latest_build_for_repo(pool: &SqlitePool, repo_id: i64) -> Result<Option<Build>> {
    let row =
        sqlx::query("SELECT * FROM builds WHERE repo_id = ? ORDER BY created_at DESC LIMIT 1")
            .bind(repo_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| row_to_build(&r)))
}

pub async fn list_builds_for_repo(
    pool: &SqlitePool,
    repo_id: i64,
    limit: i64,
) -> Result<Vec<Build>> {
    let rows =
        sqlx::query("SELECT * FROM builds WHERE repo_id = ? ORDER BY created_at DESC LIMIT ?")
            .bind(repo_id)
            .bind(limit)
            .fetch_all(pool)
            .await?;
    Ok(rows.iter().map(row_to_build).collect())
}

pub async fn list_queued_build_ids(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows = sqlx::query("SELECT id FROM builds WHERE status = ? ORDER BY created_at ASC")
        .bind(BuildStatus::Queued.as_str())
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

pub async fn recover_stale_running_builds(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE builds SET status = ?, finished_at = datetime('now'), error_summary = 'daemon restarted' WHERE status = ?",
    )
    .bind(BuildStatus::Failed.as_str())
    .bind(BuildStatus::Running.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

fn row_to_build(r: &sqlx::sqlite::SqliteRow) -> Build {
    let status: String = r.get("status");
    let store_paths: Option<String> = r.get("store_paths");
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
        store_paths: store_paths.and_then(|j| serde_json::from_str(&j).ok()),
        created_at: r.get("created_at"),
    }
}
