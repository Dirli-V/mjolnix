use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub repos_dir: PathBuf,
    pub database_url: String,
    pub work_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub host: String,
    pub stores_dir: PathBuf,
    pub cache_bind: String,
    pub cache_host: String,
    pub cache_port: u16,
    pub cache_sign_key_path: PathBuf,
    pub cache_key_name: String,
    pub max_parallel_builds: usize,
    pub build_timeout_secs: u64,
    pub mjolnix_frontend_bin: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = env::var("MJOLNIX_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|_| {
                directories::ProjectDirs::from("", "", "mjolnix")
                    .map(|d| d.data_dir().to_path_buf())
                    .ok_or_else(|| anyhow::anyhow!("could not determine data directory"))
            })
            .context("set MJOLNIX_DATA_DIR or use a standard home directory layout")?;

        let host = env::var("MJOLNIX_HOST").unwrap_or_else(|_| "localhost".into());
        let cache_bind = env::var("MJOLNIX_CACHE_BIND").unwrap_or_else(|_| "0.0.0.0:5000".into());
        let cache_host = env::var("MJOLNIX_CACHE_HOST").unwrap_or_else(|_| host.clone());
        let cache_port = env::var("MJOLNIX_CACHE_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);
        let cache_sign_key_path = env::var("MJOLNIX_CACHE_SIGN_KEY_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("cache-secret-key"));
        let cache_key_name =
            env::var("MJOLNIX_CACHE_KEY_NAME").unwrap_or_else(|_| format!("{host}-1"));
        let max_parallel_builds = env::var("MJOLNIX_MAX_PARALLEL_BUILDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        let build_timeout_secs = env::var("MJOLNIX_BUILD_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);
        let mjolnix_frontend_bin = env::var("MJOLNIX_FRONTEND_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                env::current_exe().unwrap_or_else(|_| PathBuf::from("mjolnix-frontend"))
            });
        let database_url = env::var("MJOLNIX_DATABASE_URL").context(
            "set MJOLNIX_DATABASE_URL (e.g. postgres://mjolnix:mjolnix@127.0.0.1:5432/mjolnix)",
        )?;

        let repos_dir = data_dir.join("repos");
        let stores_dir = env::var("MJOLNIX_STORES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("stores"));
        let work_dir = data_dir.join("work");
        let logs_dir = data_dir.join("logs");

        Ok(Self {
            data_dir,
            repos_dir,
            database_url,
            work_dir,
            logs_dir,
            host,
            stores_dir,
            cache_bind,
            cache_host,
            cache_port,
            cache_sign_key_path,
            cache_key_name,
            max_parallel_builds,
            build_timeout_secs,
            mjolnix_frontend_bin,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.data_dir,
            &self.repos_dir,
            &self.stores_dir,
            &self.work_dir,
            &self.logs_dir,
        ] {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn repo_disk_path(&self, namespace: &str, name: &str) -> PathBuf {
        self.repos_dir.join(namespace).join(format!("{name}.git"))
    }

    pub fn clone_url(&self, namespace: &str, name: &str) -> String {
        format!("{}:{namespace}/{name}.git", self.host)
    }

    pub fn build_log_path(&self, repo_id: i64, build_id: i64) -> PathBuf {
        self.logs_dir
            .join(repo_id.to_string())
            .join(format!("{build_id}.log"))
    }

    pub fn build_work_path(&self, repo_id: i64, rev: &str) -> PathBuf {
        self.work_dir.join(repo_id.to_string()).join(rev)
    }
}

pub fn parse_repo_path(path: &str) -> Result<(&str, &str)> {
    let path = path.trim().trim_matches('\'').trim_matches('"');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (namespace, name) = path
        .split_once('/')
        .context("repo path must be namespace/name (e.g. public/my-repo)")?;
    validate_repo_name(name)?;
    Ok((namespace, name))
}

pub fn validate_repo_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("repository name cannot be empty");
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        bail!("invalid repository name");
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("repository name cannot be empty");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("repository name must start with a letter or digit");
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        bail!("repository name may only contain letters, digits, -, _, and .");
    }
    Ok(())
}

pub fn is_repo_path_inside_root(root: &Path, repo_path: &Path) -> bool {
    repo_path.starts_with(root)
}
