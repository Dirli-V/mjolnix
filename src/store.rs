//! Per-repository isolated Nix stores.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::config::Config;
/// `local?root=…&uid=…&gid=…` store URI for a repo.
pub fn store_uri(store_root: &Path, uid: u32, gid: u32) -> String {
    format!(
        "local?root={}&uid={uid}&gid={gid}",
        store_root.to_string_lossy()
    )
}

pub fn store_root_for_repo(config: &Config, repo_id: i64) -> PathBuf {
    config.stores_dir.join(repo_id.to_string())
}

pub fn nix_store_dir(store_root: &Path) -> PathBuf {
    store_root.join("nix").join("store")
}

pub fn substituter_url(config: &Config, namespace: &str, name: &str) -> String {
    format!(
        "http://{}:{}/r/{namespace}/{name}",
        config.cache_host,
        config.cache_port,
        namespace = namespace,
        name = name
    )
}

pub async fn ensure_store_root(store_root: &Path) -> Result<()> {
    let nix_store = nix_store_dir(store_root);
    tokio::fs::create_dir_all(&nix_store)
        .await
        .with_context(|| format!("create {}", nix_store.display()))?;
    Ok(())
}

/// Find `/nix/store/{hash}-*` under a repo store root.
pub async fn find_store_path_by_hash(store_root: &Path, hash: &str) -> Result<Option<PathBuf>> {
    let nix_store = nix_store_dir(store_root);
    if !nix_store.is_dir() {
        return Ok(None);
    }

    let mut read_dir = tokio::fs::read_dir(&nix_store).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(hash) && name.contains('-') {
            return Ok(Some(PathBuf::from("/nix/store").join(name.as_ref())));
        }
    }
    Ok(None)
}

pub async fn closure_paths(store_uri: &str, result_link: &Path) -> Result<Vec<String>> {
    let output = Command::new("nix-store")
        .args(["--store", store_uri, "-qR"])
        .arg(result_link)
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

pub fn validate_repo_path_component(component: &str) -> Result<()> {
    if component.is_empty()
        || component.contains('/')
        || component.contains('\\')
        || component.contains("..")
    {
        bail!("invalid path component");
    }
    Ok(())
}

/// Base32 hash prefix from a `/nix/store/HASH-name` path.
pub fn store_path_hash(store_path: &str) -> Option<&str> {
    store_path
        .strip_prefix("/nix/store/")
        .and_then(|rest| rest.split_once('-'))
        .map(|(hash, _)| hash)
}

pub fn validate_repo_route(namespace: &str, name: &str) -> Result<()> {
    validate_repo_path_component(namespace)?;
    crate::config::validate_repo_name(name)?;
    Ok(())
}

pub fn process_uid_gid() -> (u32, u32) {
    unsafe { (libc::geteuid(), libc::getegid()) }
}
