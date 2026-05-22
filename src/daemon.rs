use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, mpsc};

use crate::build;
use crate::config::Config;
use crate::db;

pub async fn run(config: Config) -> Result<()> {
    config.ensure_dirs()?;

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;

    let recovered = db::recover_stale_running_builds(&pool).await?;
    if recovered > 0 {
        eprintln!("mjolnixd: marked {recovered} stale running build(s) as failed");
    }

    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path).context("remove stale socket")?;
    }

    let listener = UnixListener::bind(&config.socket_path)
        .with_context(|| format!("bind {}", config.socket_path.display()))?;

    eprintln!(
        "mjolnixd: listening on {} (max {} parallel builds)",
        config.socket_path.display(),
        config.max_parallel_builds
    );

    let (job_tx, job_rx) = mpsc::unbounded_channel::<i64>();
    let pool = Arc::new(pool);
    let config = Arc::new(config);
    let semaphore = Arc::new(Semaphore::new(config.max_parallel_builds));

    tokio::spawn(worker_loop(
        Arc::clone(&config),
        Arc::clone(&pool),
        job_rx,
        Arc::clone(&semaphore),
    ));

    for build_id in db::list_queued_build_ids(&pool).await? {
        let _ = job_tx.send(build_id);
    }

    loop {
        let (stream, _) = listener.accept().await.context("accept connection")?;
        let job_tx = job_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, &job_tx).await {
                eprintln!("mjolnixd: connection error: {err:#}");
            }
        });
    }
}

async fn handle_connection(stream: UnixStream, job_tx: &mpsc::UnboundedSender<i64>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let line = lines.next_line().await?.context("empty request")?;

    let response = match parse_request(&line) {
        Request::Enqueue { build_id } => {
            job_tx.send(build_id).context("worker channel closed")?;
            "ok\n".to_string()
        }
        Request::Ping => "pong\n".to_string(),
        Request::Invalid => "err invalid request\n".to_string(),
    };

    writer.write_all(response.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

enum Request {
    Enqueue { build_id: i64 },
    Ping,
    Invalid,
}

fn parse_request(line: &str) -> Request {
    let line = line.trim();
    if line == "ping" {
        return Request::Ping;
    }
    let Some((_cmd, id)) = line.split_once(' ') else {
        return Request::Invalid;
    };
    let Ok(build_id) = id.trim().parse::<i64>() else {
        return Request::Invalid;
    };
    Request::Enqueue { build_id }
}

async fn worker_loop(
    config: Arc<Config>,
    pool: Arc<SqlitePool>,
    mut job_rx: mpsc::UnboundedReceiver<i64>,
    semaphore: Arc<Semaphore>,
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
            if let Err(err) = run_one_build(&config, &pool, build_id).await {
                eprintln!("mjolnixd: build {build_id} failed: {err:#}");
            }
        });
    }
}

async fn run_one_build(config: &Config, pool: &SqlitePool, build_id: i64) -> Result<()> {
    let build = db::get_build(pool, build_id)
        .await?
        .context("build not found")?;

    if build.status != db::BuildStatus::Queued {
        return Ok(());
    }

    let repo_row = sqlx::query("SELECT namespace, name FROM repos WHERE id = ?")
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

    build::run_build(config, pool, &build, &repo).await
}

pub async fn enqueue_build(config: &Config, build_id: i64) -> Result<()> {
    let mut stream = UnixStream::connect(&config.socket_path)
        .await
        .with_context(|| {
            format!(
                "connect to mjolnixd at {} (is mjolnixd running?)",
                config.socket_path.display()
            )
        })?;

    let request = format!("enqueue {build_id}\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let response = lines.next_line().await?.unwrap_or_default();

    if response.trim() != "ok" {
        bail!("mjolnixd enqueue failed: {response}");
    }

    Ok(())
}
