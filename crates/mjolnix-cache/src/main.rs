use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::extract::{Path as AxumPath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::{self, DbPool};
use mjolnix_shared::signing::{self, CacheSigningKey};
use mjolnix_shared::store::{self, RepoStore};
use serde::Deserialize;
use tokio::process::Command;

#[derive(Clone)]
struct CacheState {
    pool: DbPool,
    config: Arc<Config>,
    signing_key: Arc<CacheSigningKey>,
    store_ids: store::NixStoreIds,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;

    let signing_key =
        signing::load_or_create_secret_key(&config.cache_sign_key_path, &config.cache_key_name)
            .await?;
    let store_ids = store::NixStoreIds::current();

    run_server(&config, pool, signing_key, store_ids).await
}

async fn run_server(
    config: &Config,
    pool: DbPool,
    signing_key: CacheSigningKey,
    store_ids: store::NixStoreIds,
) -> Result<()> {
    let state = CacheState {
        pool,
        config: Arc::new(config.clone()),
        signing_key: Arc::new(signing_key),
        store_ids,
    };
    let app = Router::new()
        .route("/r/{namespace}/{name}/{*rest}", routing::get(serve))
        .with_state(state);
    let addr: SocketAddr = config
        .cache_bind
        .parse()
        .with_context(|| format!("parse cache bind {}", config.cache_bind))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind binary cache on {addr}"))?;
    eprintln!("mjolnix-cache: listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .context("binary cache server")?;
    Ok(())
}

async fn serve(
    State(state): State<CacheState>,
    AxumPath((namespace, name, rest)): AxumPath<(String, String, String)>,
) -> Result<Response, CacheError> {
    store::validate_repo_route(&namespace, &name).map_err(CacheError::bad_request)?;
    let repo = db::get_repo(&state.pool, &namespace, &name)
        .await
        .map_err(CacheError::internal)?
        .ok_or_else(|| CacheError::not_found("repository not found"))?;
    let repo_store = store::repo_store(
        &state.config,
        &repo,
        state.store_ids,
        Some(state.signing_key.public_key_line.clone()),
    );

    let rest = rest.trim_start_matches('/');
    if rest.ends_with(".narinfo") {
        let hash = rest.strip_suffix(".narinfo").unwrap_or(rest);
        return serve_narinfo(&state, &repo_store, hash).await;
    }
    if let Some(hash) = rest
        .strip_prefix("nar/")
        .and_then(|s| s.strip_suffix(".nar.xz"))
    {
        return serve_nar(&repo_store, hash).await;
    }
    Err(CacheError::not_found("unknown cache path"))
}

async fn serve_narinfo(
    state: &CacheState,
    repo_store: &RepoStore,
    hash: &str,
) -> Result<Response, CacheError> {
    let store_path = store::find_store_path_by_hash(Path::new(&repo_store.store_root), hash)
        .await
        .map_err(CacheError::internal)?
        .ok_or_else(|| CacheError::not_found("store path not found"))?;
    let store_path_str = store_path.to_string_lossy();
    let info = path_info(&repo_store.store_uri, &store_path_str)
        .await
        .map_err(CacheError::internal)?;
    let path_hash = store::store_path_hash(&info.store_path).unwrap_or(hash);
    let file_hash = nar_file_hash(&repo_store.store_uri, &store_path_str)
        .await
        .map_err(CacheError::internal)?;
    let refs: Vec<_> = info
        .references
        .iter()
        .filter(|r| r.as_str() != info.store_path)
        .collect();

    let mut body = String::new();
    body.push_str(&format!("StorePath: {}\n", info.store_path));
    if let Some(deriver) = &info.deriver {
        body.push_str(&format!("Deriver: {deriver}\n"));
    }
    body.push_str(&format!("URL: nar/{path_hash}.nar.xz\n"));
    body.push_str("Compression: xz\n");
    body.push_str(&format!("FileHash: {file_hash}\n"));
    body.push_str(&format!("NarHash: {}\n", info.nar_hash));
    body.push_str(&format!("NarSize: {}\n", info.nar_size));
    if !refs.is_empty() {
        body.push_str(&format!(
            "References: {}\n",
            refs.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    let sig = state.signing_key.sign_narinfo(&body);
    body.push_str(&format!("Sig: {sig}\n"));

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/x-nix-narinfo")],
        body,
    )
        .into_response())
}

async fn serve_nar(repo_store: &RepoStore, hash: &str) -> Result<Response, CacheError> {
    let store_path = store::find_store_path_by_hash(Path::new(&repo_store.store_root), hash)
        .await
        .map_err(CacheError::internal)?
        .ok_or_else(|| CacheError::not_found("store path not found"))?;
    let store_path_str = store_path.to_string_lossy();
    let script = format!(
        "exec nix-store --store '{}' --dump '{}' | xz -c",
        repo_store.store_uri.replace('\'', "'\\''"),
        store_path_str.replace('\'', "'\\''")
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .output()
        .await
        .map_err(CacheError::internal)?;
    if !output.status.success() {
        return Err(CacheError::internal(anyhow::anyhow!(
            "nar export failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-nix-archive")],
        output.stdout,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct PathInfoEntry {
    #[serde(rename = "narHash")]
    nar_hash: String,
    #[serde(rename = "narSize")]
    nar_size: u64,
    deriver: Option<String>,
    #[serde(default)]
    references: Vec<String>,
}

#[derive(Debug)]
struct PathInfo {
    store_path: String,
    nar_hash: String,
    nar_size: u64,
    deriver: Option<String>,
    references: Vec<String>,
}

async fn path_info(store_uri: &str, store_path: &str) -> Result<PathInfo> {
    let output = Command::new("nix")
        .args(["path-info", "--store", store_uri, "--json"])
        .arg(store_path)
        .output()
        .await
        .context("nix path-info")?;
    if !output.status.success() {
        bail!(
            "nix path-info failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&output.stdout).context("parse path-info json")?;
    let (path, value) = map.into_iter().next().context("empty path-info response")?;
    let entry: PathInfoEntry = serde_json::from_value(value).context("parse path-info entry")?;
    Ok(PathInfo {
        store_path: path,
        nar_hash: entry.nar_hash,
        nar_size: entry.nar_size,
        deriver: entry.deriver,
        references: entry.references,
    })
}

async fn nar_file_hash(store_uri: &str, store_path: &str) -> Result<String> {
    let script = format!(
        "nix-store --store '{}' --dump '{}' | xz -c | nix hash file --sri --type sha256",
        store_uri.replace('\'', "'\\''"),
        store_path.replace('\'', "'\\''")
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .output()
        .await
        .context("nar file hash pipeline")?;
    if !output.status.success() {
        bail!(
            "nar file hash failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug)]
struct CacheError {
    status: StatusCode,
    message: String,
}

impl CacheError {
    fn not_found(msg: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    fn bad_request(err: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: err.to_string(),
        }
    }
    fn internal(err: impl Into<anyhow::Error>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.into().to_string(),
        }
    }
}

impl IntoResponse for CacheError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}
