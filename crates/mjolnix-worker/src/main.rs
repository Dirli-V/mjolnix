use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, Build, BuildStatus, DbPool};
use mjolnix_shared::signing::{self, CacheSigningKey};
use mjolnix_shared::store;
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

    let cache_signing_key =
        signing::load_or_create_secret_key(&config.cache_sign_key_path, &config.cache_key_name)
            .await?;

    let config = Arc::new(config);
    let pool = Arc::new(pool);
    let cache_signing_key = Arc::new(cache_signing_key);
    let semaphore = Arc::new(Semaphore::new(config.max_parallel_builds));
    let (slot_free_tx, mut slot_free_rx) = mpsc::unbounded_channel::<()>();
    let store_ids = store::NixStoreIds::current();

    spawn_stale_build_checker(Arc::clone(&pool));

    let mut listener = db::connect_listener(&config.database_url)
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
            Arc::clone(&cache_signing_key),
            slot_free_tx.clone(),
            store_ids,
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
    cache_signing_key: Arc<CacheSigningKey>,
    slot_free_tx: mpsc::UnboundedSender<()>,
    store_ids: store::NixStoreIds,
) -> Result<()> {
    loop {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .context("semaphore")?;
        let build = db::claim_next_queued_build(&pool).await?;
        let Some(build) = build else {
            drop(permit);
            break;
        };
        tokio::spawn(run_build_task(
            config.clone(),
            pool.clone(),
            cache_signing_key.clone(),
            build,
            permit,
            slot_free_tx.clone(),
            store_ids,
        ));
    }
    Ok(())
}

fn spawn_stale_build_checker(pool: Arc<DbPool>) {
    tokio::spawn(async move {
        let mut interval =
            time::interval(time::Duration::from_secs(db::BUILD_HEARTBEAT_INTERVAL_SECS));
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
    cache_signing_key: Arc<CacheSigningKey>,
    build: Build,
    permit: tokio::sync::OwnedSemaphorePermit,
    slot_free_tx: mpsc::UnboundedSender<()>,
    store_ids: store::NixStoreIds,
) {
    let build_id = build.id;
    let heartbeat = spawn_build_heartbeat(Arc::clone(&pool), build_id);
    if let Err(err) = run_one_build(
        &config,
        &pool,
        &build,
        store_ids,
        Some(cache_signing_key.public_key_line.as_str()),
    )
    .await
    {
        eprintln!("mjolnix-worker: build {build_id} failed: {err:#}");
    }
    heartbeat.abort();
    drop(permit);
    let _ = slot_free_tx.send(());
}

fn spawn_build_heartbeat(pool: Arc<DbPool>, build_id: i64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            time::interval(time::Duration::from_secs(db::BUILD_HEARTBEAT_INTERVAL_SECS));
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
    store_ids: store::NixStoreIds,
    cache_public_key: Option<&str>,
) -> Result<()> {
    if build.log_path.is_some() {
        return Ok(());
    }

    let current = db::get_build(pool, build.id).await?;
    if current.as_ref().map(|b| b.status) != Some(BuildStatus::Running) {
        return Ok(());
    }

    let Some(repo) = db::get_repo_by_id(pool, build.repo_id).await? else {
        bail!("repo {} not found for build {}", build.repo_id, build.id);
    };
    build::run_build(config, pool, build, &repo, store_ids, cache_public_key).await
}
