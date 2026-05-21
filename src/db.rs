use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use crate::config::Config;

pub async fn connect(config: &Config) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .create_if_missing(true);

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

#[derive(Debug, Clone)]
pub struct Repo {
    pub namespace: String,
    pub name: String,
}

pub async fn list_repos_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Repo>> {
    let rows = sqlx::query(
        "SELECT namespace, name FROM repos WHERE user_id = ? ORDER BY namespace, name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Repo {
            namespace: r.get("namespace"),
            name: r.get("name"),
        })
        .collect())
}

pub async fn create_repo(
    pool: &SqlitePool,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO repos (user_id, namespace, name) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(namespace)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn user_owns_repo(
    pool: &SqlitePool,
    user_id: i64,
    namespace: &str,
    name: &str,
) -> Result<bool> {
    let row = sqlx::query(
        "SELECT 1 AS ok FROM repos WHERE user_id = ? AND namespace = ? AND name = ?",
    )
    .bind(user_id)
    .bind(namespace)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}
