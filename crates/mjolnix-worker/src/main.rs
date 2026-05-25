use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, Build, BuildStatus, DbPool};
use mjolnix_shared::signing;
use mjolnix_shared::store;
use sqlx::postgres::PgListener;
use sqlx::Row;
use tokio::sync::{Semaphore, mpsc};
use tokio::time;

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

    signing::publish_cache_public_keys(&config, &pool).await?;

    let config = Arc::new(config);
    let pool = Arc::new(pool);
    let semaphore = Arc::new(Semaphore::new(config.max_parallel_builds));
    let (slot_free_tx, mut slot_free_rx) = mpsc::unbounded_channel::<()>();
    let (uid, gid) = store::process_uid_gid();

    spawn_stale_build_checker(Arc::clone(&pool));

    let mut listener = PgListener::connect(&config.database_url)
        .await
        .context("connect for LISTEN on build queue")?;
    listener
        .listen(db::BUILD_QUEUED_CHANNEL)
        .await
        .context("LISTEN on build queue channel")?;

    eprintln!(
        "mjolnix-worker: running (max {} parallel builds)",
        config.max_parallel_builds
    );

    loop {
        fill_slots(
            Arc::clone(&config),
            Arc::clone(&pool),
            Arc::clone(&semaphore),
            slot_free_tx.clone(),
            uid,
            gid,
        )
        .await?;

        tokio::select! {
            result = listener.recv() => {
                result.context("receive build queue notification")?;
            }
            msg = slot_free_rx.recv() => {
                if msg.is_none() {
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Claim and start builds only while a semaphore slot is available (never hoards queued rows).
async fn fill_slots(
    config: Arc<Config>,
    pool: Arc<DbPool>,
    semaphore: Arc<Semaphore>,
    slot_free_tx: mpsc::UnboundedSender<()>,
    uid: u32,
    gid: u32,
) -> Result<()> {
    loop {
        let permit = semaphore.clone().acquire_owned().await.context("semaphore")?;
        let build = db::claim_next_queued_build(&pool).await?;
        let Some(build) = build else {
            drop(permit);
            break;
        };
        tokio::spawn(run_build_task(
            config.clone(),
            pool.clone(),
            build,
            permit,
            slot_free_tx.clone(),
            uid,
            gid,
        ));
    }
    Ok(())
}

fn spawn_stale_build_checker(pool: Arc<DbPool>) {
    tokio::spawn(async move {
        let mut interval = time::interval(time::Duration::from_secs(
            db::BUILD_HEARTBEAT_INTERVAL_SECS,
        ));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match db::fail_stale_running_builds(&pool).await {
                Ok(n) if n > 0 => {
                    eprintln!("mjolnix-worker: marked {n} stale running build(s) as failed");
                }
                Ok(_) => {}
                Err(err) => eprintln!("mjolnix-worker: stale build check failed: {err:#}"),
            }
        }
    });
}

async fn run_build_task(
    config: Arc<Config>,
    pool: Arc<DbPool>,
    build: Build,
    permit: tokio::sync::OwnedSemaphorePermit,
    slot_free_tx: mpsc::UnboundedSender<()>,
    uid: u32,
    gid: u32,
) {
    let _permit = permit;
    let build_id = build.id;
    let heartbeat = spawn_build_heartbeat(Arc::clone(&pool), build_id);
    if let Err(err) = run_one_build(&config, &pool, &build, uid, gid).await {
        eprintln!("mjolnix-worker: build {build_id} failed: {err:#}");
    }
    heartbeat.abort();
    let _ = slot_free_tx.send(());
}

fn spawn_build_heartbeat(pool: Arc<DbPool>, build_id: i64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(time::Duration::from_secs(
            db::BUILD_HEARTBEAT_INTERVAL_SECS,
        ));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if db::touch_build_heartbeat(&pool, build_id).await.is_err() {
                break;
            }
        }
    })
}

async fn run_one_build(
    config: &Config,
    pool: &DbPool,
    build: &Build,
    uid: u32,
    gid: u32,
) -> Result<()> {
    if build.log_path.is_some() {
        return Ok(());
    }

    let current = db::get_build(pool, build.id).await?;
    if current.as_ref().map(|b| b.status) != Some(BuildStatus::Running) {
        return Ok(());
    }

    let repo_row = sqlx::query("SELECT namespace, name FROM repos WHERE id = $1")
        .bind(build.repo_id)
        .fetch_optional(pool)
        .await?;
    let Some(repo_row) = repo_row else {
        bail!("repo {} not found for build {}", build.repo_id, build.id);
    };
    let repo = db::Repo {
        id: build.repo_id,
        namespace: repo_row.get("namespace"),
        name: repo_row.get("name"),
    };
    build::run_build(config, pool, build, &repo, uid, gid).await
}
