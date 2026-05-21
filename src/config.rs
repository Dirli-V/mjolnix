use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub repos_dir: PathBuf,
    pub db_path: PathBuf,
    /// Shown in clone URLs (e.g. `my-host.com:public/foo.git`).
    pub host: String,
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

        let repos_dir = data_dir.join("repos");
        let db_path = data_dir.join("mjolnix.db");

        Ok(Self {
            data_dir,
            repos_dir,
            db_path,
            host,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("create data dir {}", self.data_dir.display()))?;
        std::fs::create_dir_all(&self.repos_dir)
            .with_context(|| format!("create repos dir {}", self.repos_dir.display()))?;
        Ok(())
    }

    pub fn repo_disk_path(&self, namespace: &str, name: &str) -> PathBuf {
        self.repos_dir
            .join(namespace)
            .join(format!("{name}.git"))
    }

    pub fn clone_url(&self, namespace: &str, name: &str) -> String {
        format!("{}:{namespace}/{name}.git", self.host)
    }
}

/// `public/my-repo` or `public/my-repo.git` → (`public`, `my-repo`)
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
