use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, Build, DbPool, Repo};
use mjolnix_shared::store::RepoStore;
use mjolnix_shared::store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

pub async fn run_build(
    config: &Config,
    pool: &DbPool,
    build: &Build,
    repo: &Repo,
    store_ids: store::NixStoreIds,
    cache_public_key: Option<&str>,
) -> Result<()> {
    let log_path = config.build_log_path(build.repo_id, build.id);
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create log dir {}", parent.display()))?;
    }

    db::set_build_running(pool, build.id, &log_path.to_string_lossy()).await?;

    let store_root = store::store_root_for_repo(config, repo.id);
    store::ensure_store_root(&store_root).await?;
    let repo_store = store::repo_store(
        config,
        repo,
        store_ids,
        cache_public_key.map(str::to_string),
    );
    let repo_path = config.repo_disk_path(&repo.namespace, &repo.name);
    let work_path = config.build_work_path(build.repo_id, &build.rev);

    if let Err(err) = run_build_inner(
        config,
        &repo_store,
        &repo_path,
        &build.rev,
        &work_path,
        &log_path,
    )
    .await
    {
        let summary = err.to_string();
        let _ = append_log(&log_path, &format!("\n--- build failed ---\n{summary}\n")).await;
        db::set_build_failed(pool, build.id, &truncate_summary(&summary)).await?;
        return Err(err);
    }

    let result_link = work_path.join("result");
    let paths = store::closure_paths(&repo_store.store_uri, &result_link).await?;
    db::set_build_success(pool, build.id, &paths).await?;
    Ok(())
}

async fn run_build_inner(
    config: &Config,
    repo_store: &RepoStore,
    repo_path: &Path,
    rev: &str,
    work_path: &Path,
    log_path: &Path,
) -> Result<()> {
    if work_path.exists() {
        tokio::fs::remove_dir_all(work_path)
            .await
            .with_context(|| format!("remove old workdir {}", work_path.display()))?;
    }
    tokio::fs::create_dir_all(work_path)
        .await
        .with_context(|| format!("create workdir {}", work_path.display()))?;
    materialize_rev(repo_path, rev, work_path).await?;

    let mut log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
        .await
        .context("open build log")?;

    log_file
        .write_all(format!("--- nix build (store: {}) ---\n", repo_store.store_uri).as_bytes())
        .await
        .context("write log")?;
    log_file
        .write_all(format!("--- substituter: {} ---\n", repo_store.substituter_url).as_bytes())
        .await
        .ok();

    let flake_path = format!("{}#", work_path.display());
    let result_link = work_path.join("result");
    let mut cmd = Command::new("nix");
    cmd.args([
        "build",
        &flake_path,
        "--out-link",
        result_link.to_string_lossy().as_ref(),
        "--store",
        &repo_store.store_uri,
    ])
    .current_dir(work_path)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    if let Some(ref public_key) = repo_store.cache_public_key {
        cmd.args([
            "--option",
            "substituters",
            &repo_store.substituter_url,
            "--option",
            "trusted-public-keys",
            public_key,
        ]);
    } else {
        log_file
            .write_all(b"warning: cache enabled but cache_public_key not set yet\n")
            .await
            .ok();
    }

    let mut child = cmd.spawn().context("spawn nix build")?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let build_fut = async {
        let read_out = async {
            let mut buf = Vec::new();
            if let Some(out) = stdout.as_mut() {
                out.read_to_end(&mut buf)
                    .await
                    .context("read nix stdout")?;
            }
            Ok::<Vec<u8>, anyhow::Error>(buf)
        };
        let read_err = async {
            let mut buf = Vec::new();
            if let Some(err) = stderr.as_mut() {
                err.read_to_end(&mut buf)
                    .await
                    .context("read nix stderr")?;
            }
            Ok::<Vec<u8>, anyhow::Error>(buf)
        };
        let (status, stdout_bytes, stderr_bytes) =
            tokio::join!(child.wait(), read_out, read_err);
        let status = status.context("wait nix build")?;
        Ok::<_, anyhow::Error>((status, stdout_bytes?, stderr_bytes?))
    };

    let (status, stdout_bytes, stderr_bytes): (std::process::ExitStatus, Vec<u8>, Vec<u8>) = timeout(
        Duration::from_secs(config.build_timeout_secs),
        build_fut,
    )
    .await
    .context("build timed out")?
    .context("run nix build")?;

    log_file.write_all(&stdout_bytes).await.ok();
    log_file.write_all(&stderr_bytes).await.ok();

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        bail!(
            "nix build failed (exit {:?}): {}",
            status.code(),
            truncate_summary(&stderr)
        );
    }
    Ok(())
}

async fn materialize_rev(repo_path: &Path, rev: &str, work_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["--git-dir", &repo_path.to_string_lossy(), "archive", rev])
        .stdout(Stdio::piped())
        .output()
        .await
        .context("git archive")?;
    if !output.status.success() {
        bail!(
            "git archive failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut tar = Command::new("tar");
    tar.args(["-x", "-C"])
        .arg(work_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = tar.spawn().context("spawn tar")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&output.stdout)
            .await
            .context("write tar stdin")?;
    }
    let status = child.wait().await.context("wait tar")?;
    if !status.success() {
        bail!("tar extract failed");
    }
    Ok(())
}

async fn append_log(log_path: &Path, text: &str) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await?;
    file.write_all(text.as_bytes()).await?;
    Ok(())
}

fn truncate_summary(s: &str) -> String {
    const MAX: usize = 500;
    let line = s.lines().last().unwrap_or(s).trim();
    if line.len() <= MAX {
        line.to_string()
    } else {
        format!("{}...", &line[..MAX])
    }
}
