use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, Build, BuildStatus, DbPool};
use mjolnix_shared::signing::{self, CacheSigningKey};
use mjolnix_shared::{DbListener, store};
use tokio::sync::{Semaphore, mpsc};
use tokio::time;

mod build;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;
    run(config).await
}

pub(crate) struct Ctx {
    pool: DbPool,
    cache_signing_key: CacheSigningKey,
    config: Config,
    semaphore: Semaphore,
    slot_free_tx: mpsc::UnboundedSender<()>,
    store_ids: store::NixStoreIds,
}

impl Ctx {
    async fn new(config: Config, slot_free_tx: mpsc::UnboundedSender<()>) -> Result<Self> {
        let pool = db::connect(&config).await?;
        let cache_signing_key =
            signing::load_or_create_secret_key(&config.cache_sign_key_path, &config.cache_key_name)
                .await?;
        let semaphore = Semaphore::new(config.max_parallel_builds);
        let store_ids = store::NixStoreIds::current();
        Ok(Self {
            pool,
            cache_signing_key,
            config,
            semaphore,
            slot_free_tx,
            store_ids,
        })
    }
}

async fn run(config: Config) -> Result<()> {
    let (slot_free_tx, mut slot_free_rx) = mpsc::unbounded_channel::<()>();
    let ctx = Arc::new(Ctx::new(config, slot_free_tx).await?);
    let stale_build_handler = spawn_stale_build_checker(ctx.clone());
    let mut listener = create_build_listener(&ctx).await?;
    loop {
        fill_slots(Arc::clone(&ctx)).await?;
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

    stale_build_handler.abort();
    Ok(())
}

async fn create_build_listener(ctx: &Ctx) -> Result<DbListener> {
    let mut listener = db::connect_listener(&ctx.config.database_url)
        .await
        .context("connect for LISTEN on build queue")?;
    listener
        .listen(db::BUILD_QUEUED_CHANNEL)
        .await
        .context("LISTEN on build queue channel")?;
    Ok(listener)
}

async fn fill_slots(ctx: Arc<Ctx>) -> Result<()> {
    loop {
        let permit = ctx.semaphore.acquire().await?;
        let build = db::claim_next_queued_build(&ctx.pool).await?;
        let Some(build) = build else {
            break;
        };
        tokio::spawn(run_build_task(ctx.clone(), build));
        drop(permit);
        let _ = ctx.slot_free_tx.send(());
    }
    Ok(())
}

fn spawn_stale_build_checker(ctx: Arc<Ctx>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            time::interval(time::Duration::from_secs(db::BUILD_HEARTBEAT_INTERVAL_SECS));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match db::fail_stale_running_builds(&ctx.pool).await {
                Ok(n) if n > 0 => {
                    eprintln!("mjolnix-worker: marked {n} stale running build(s) as failed");
                }
                Ok(_) => {}
                Err(err) => eprintln!("mjolnix-worker: stale build check failed: {err:#}"),
            }
        }
    })
}

async fn run_build_task(ctx: Arc<Ctx>, build: Build) {
    let build_id = build.id;
    let heartbeat = spawn_build_heartbeat(ctx.clone(), build_id);
    if let Err(err) = run_one_build(&ctx, &build).await {
        eprintln!("mjolnix-worker: build {build_id} failed: {err:#}");
    }
    heartbeat.abort();
}

fn spawn_build_heartbeat(ctx: Arc<Ctx>, build_id: i64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            time::interval(time::Duration::from_secs(db::BUILD_HEARTBEAT_INTERVAL_SECS));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if db::touch_build_heartbeat(&ctx.pool, build_id)
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

async fn run_one_build(ctx: &Ctx, build: &Build) -> Result<()> {
    if build.log_path.is_some() {
        return Ok(());
    }

    let current = db::get_build(&ctx.pool, build.id).await?;
    if current.as_ref().map(|b| b.status) != Some(BuildStatus::Running) {
        return Ok(());
    }

    let Some(repo) = db::get_repo_by_id(&ctx.pool, build.repo_id).await? else {
        bail!("repo {} not found for build {}", build.repo_id, build.id);
    };
    build::run_build(ctx, build, &repo).await
}
