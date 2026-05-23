use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::db::DbPool;
use anyhow::{Context, Result};

use crate::config::Config;
use crate::daemon;
use crate::db;

const POST_RECEIVE_HOOK: &str = r#"#!/bin/sh
# mjolnix post-receive — do not edit manually
set -e
MJOLNIX_BIN="@MJOLNIX_BIN@"
NS="@NAMESPACE@"
NAME="@NAME@"
while read -r old new ref; do
  "$MJOLNIX_BIN" hook-post-receive "$NS" "$NAME" "$old" "$new" "$ref" &
done
wait
"#;

pub fn install_post_receive_hook(config: &Config, namespace: &str, name: &str) -> Result<()> {
    let repo_path = config.repo_disk_path(namespace, name);
    let hooks_dir = repo_path.join("hooks");
    fs::create_dir_all(&hooks_dir).context("create hooks directory")?;

    let script = POST_RECEIVE_HOOK
        .replace("@MJOLNIX_BIN@", &config.mjolnix_bin.to_string_lossy())
        .replace("@NAMESPACE@", namespace)
        .replace("@NAME@", name);

    let hook_path = hooks_dir.join("post-receive");
    fs::write(&hook_path, script).context("write post-receive hook")?;
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
        .context("chmod post-receive hook")?;
    Ok(())
}

pub fn repo_has_flake(repo_path: &Path, rev: &str) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args([
            "--git-dir",
            &repo_path.to_string_lossy(),
            "cat-file",
            "-e",
            &format!("{rev}:flake.nix"),
        ])
        .output()
        .context("git cat-file flake.nix")?;
    Ok(output.status.success())
}

pub async fn hook_post_receive(
    config: &Config,
    pool: &DbPool,
    namespace: &str,
    name: &str,
    old: &str,
    new: &str,
    ref_name: &str,
) -> Result<()> {
    let _ = old;

    if new.chars().all(|c| c == '0') {
        return Ok(());
    }

    let repo = db::get_repo(pool, namespace, name)
        .await?
        .context("repository not in database")?;

    let repo_path = config.repo_disk_path(namespace, name);
    if !repo_has_flake(&repo_path, new)? {
        return Ok(());
    }

    let build_id = db::insert_build_queued(pool, repo.id, new, ref_name).await?;

    if let Err(err) = daemon::enqueue_build(config, build_id).await {
        eprintln!("mjolnix: could not notify mjolnixd (build {build_id} stays queued): {err:#}");
    }

    Ok(())
}
