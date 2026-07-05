use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct CacheSigningKey {
    pub name: String,
    pub public_key_line: String,
    signing_key: SigningKey,
}

impl CacheSigningKey {
    pub fn sign_narinfo(&self, body: &str) -> String {
        use ed25519_dalek::Signer;
        let sig = self.signing_key.sign(body.as_bytes());
        format!("{}:{}", self.name, STANDARD.encode(sig.to_bytes()))
    }
}

pub async fn load_secret_key(path: &Path) -> Result<CacheSigningKey> {
    parse_secret_key_file(path).await
}

pub async fn load_or_create_secret_key(path: &Path, key_name: &str) -> Result<CacheSigningKey> {
    if !path.exists() {
        ensure_parent(path)?;
        let output = Command::new("nix")
            .args(["key", "generate-secret", "--key-name", key_name])
            .output()
            .await
            .context("nix key generate-secret")?;
        if !output.status.success() {
            bail!(
                "nix key generate-secret failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::write(path, &output.stdout).with_context(|| format!("write {}", path.display()))?;
    }

    parse_secret_key_file(path).await
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

async fn parse_secret_key_file(path: &Path) -> Result<CacheSigningKey> {
    let line = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?
        .lines()
        .find(|l| !l.trim().is_empty())
        .context("empty signing key file")?
        .trim()
        .to_string();

    let (name, secret_b64) = line
        .split_once(':')
        .context("signing key must be name:base64")?;
    let secret_bytes = STANDARD
        .decode(secret_b64.as_bytes())
        .context("decode secret key base64")?;

    let signing_key = if secret_bytes.len() == 64 {
        SigningKey::from_keypair_bytes(secret_bytes.as_slice().try_into().expect("length checked"))
            .context("invalid signing keypair bytes")?
    } else if secret_bytes.len() == 32 {
        SigningKey::from_bytes(secret_bytes.as_slice().try_into().expect("length checked"))
    } else {
        bail!("unexpected signing key length {}", secret_bytes.len());
    };

    let public_key_line = public_key_from_secret(path).await?;
    Ok(CacheSigningKey {
        name: name.to_string(),
        public_key_line,
        signing_key,
    })
}

async fn public_key_from_secret(path: &Path) -> Result<String> {
    let secret_text = fs::read_to_string(path)?;
    let mut child = Command::new("nix")
        .args(["key", "convert-secret-to-public"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn nix key convert-secret-to-public")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(secret_text.as_bytes()).await?;
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        bail!(
            "nix key convert-secret-to-public failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|l| !l.trim().is_empty())
        .context("empty public key output")?
        .trim()
        .to_string())
}
