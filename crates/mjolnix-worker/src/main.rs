use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, BuildStatus, DbPool};
use mjolnix_shared::signing;
use mjolnix_shared::store;
use sqlx::Row;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::{Duration, sleep};

mod build;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = run(Config::from_env()?).await {
        eprintln!("mjolnix-worker: {err:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run(config: Config) -> Result<()> {
    config.ensure_dirs()?;
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;

    let requeued = db::requeue_stale_running_builds(&pool).await?;
    if requeued > 0 {
        eprintln!("mjolnix-worker: requeued {requeued} stale running build(s) after restart");
    }

    signing::publish_cache_public_keys(&config, &pool).await?;

    let config = Arc::new(config);
    let pool = Arc::new(pool);
    let semaphore = Arc::new(Semaphore::new(config.max_parallel_builds));
    let (uid, gid) = store::process_uid_gid();
    let (job_tx, job_rx) = mpsc::unbounded_channel::<i64>();

    tokio::spawn(worker_loop(
        Arc::clone(&config),
        Arc::clone(&pool),
        job_rx,
        Arc::clone(&semaphore),
        uid,
        gid,
    ));

    eprintln!(
        "mjolnix-worker: running (max {} parallel builds)",
        config.max_parallel_builds
    );

    loop {
        for build_id in db::list_queued_build_ids(&pool).await? {
            let _ = job_tx.send(build_id);
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn worker_loop(
    config: Arc<Config>,
    pool: Arc<DbPool>,
    mut job_rx: mpsc::UnboundedReceiver<i64>,
    semaphore: Arc<Semaphore>,
    uid: u32,
    gid: u32,
) {
    while let Some(build_id) = job_rx.recv().await {
        let config = Arc::clone(&config);
        let pool = Arc::clone(&pool);
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = run_one_build(&config, &pool, build_id, uid, gid).await {
                eprintln!("mjolnix-worker: build {build_id} failed: {err:#}");
            }
        });
    }
}

async fn run_one_build(
    config: &Config,
    pool: &DbPool,
    build_id: i64,
    uid: u32,
    gid: u32,
) -> Result<()> {
    let build = db::get_build(pool, build_id)
        .await?
        .context("build not found")?;
    if build.status != BuildStatus::Queued {
        return Ok(());
    }

    let repo_row = sqlx::query("SELECT namespace, name FROM repos WHERE id = $1")
        .bind(build.repo_id)
        .fetch_optional(pool)
        .await?;
    let Some(repo_row) = repo_row else {
        bail!("repo {} not found for build {}", build.repo_id, build_id);
    };
    let repo = db::Repo {
        id: build.repo_id,
        namespace: repo_row.get("namespace"),
        name: repo_row.get("name"),
    };
    build::run_build(config, pool, &build, &repo, uid, gid).await
}
