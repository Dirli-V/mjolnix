use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use sqlx::sqlite::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::config::Config;
use crate::db::{self, Build, Repo};

pub async fn run_build(
    config: &Config,
    pool: &SqlitePool,
    build: &Build,
    repo: &Repo,
) -> Result<()> {
    let log_path = config.build_log_path(build.repo_id, build.id);
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create log dir {}", parent.display()))?;
    }

    db::set_build_running(pool, build.id, &log_path.to_string_lossy()).await?;

    let repo_path = config.repo_disk_path(&repo.namespace, &repo.name);
    let work_path = config.build_work_path(build.repo_id, &build.rev);

    if let Err(err) = run_build_inner(config, &repo_path, &build.rev, &work_path, &log_path).await {
        let summary = err.to_string();
        let _ = append_log(&log_path, &format!("\n--- build failed ---\n{summary}\n")).await;
        db::set_build_failed(pool, build.id, &truncate_summary(&summary)).await?;
        return Err(err);
    }

    let result_link = work_path.join("result");
    let store_paths = closure_paths(&result_link).await?;
    db::set_build_success(pool, build.id, &store_paths).await?;
    Ok(())
}

async fn run_build_inner(
    config: &Config,
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
        .write_all(b"--- nix build ---\n")
        .await
        .context("write log")?;

    let flake_path = format!("{}#", work_path.display());
    let result_link = work_path.join("result");

    let mut cmd = Command::new("nix");
    cmd.args(["build", &flake_path, "--out-link"])
        .arg(&result_link)
        .current_dir(work_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let duration = Duration::from_secs(config.build_timeout_secs);
    let output = timeout(duration, cmd.output())
        .await
        .context("build timed out")?
        .context("run nix build")?;

    log_file.write_all(&output.stdout).await.ok();
    log_file.write_all(&output.stderr).await.ok();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "nix build failed (exit {:?}): {}",
            output.status.code(),
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

async fn closure_paths(result_link: &Path) -> Result<Vec<String>> {
    let output = Command::new("nix-store")
        .args(["-qR", &result_link.to_string_lossy()])
        .output()
        .await
        .context("nix-store -qR")?;

    if !output.status.success() {
        bail!(
            "nix-store -qR failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    Ok(paths)
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
        format!("{}…", &line[..MAX])
    }
}
