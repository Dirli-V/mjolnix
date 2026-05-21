use std::borrow::Cow;
use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::auth;
use crate::config::{self, Config};
use crate::db;
use crate::hook;

pub fn remote_git_command() -> Option<String> {
    if let Ok(cmd) = env::var("SSH_ORIGINAL_COMMAND") {
        if !cmd.is_empty() {
            return Some(cmd);
        }
    }

    let mut args = env::args_os();
    args.next();
    let first = args.next()?;
    if first != "-c" {
        return None;
    }
    let rest: Vec<OsString> = args.collect();
    if rest.is_empty() {
        return None;
    }
    Some(
        rest.iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub async fn run(config: &Config, pool: &SqlitePool, command: &str) -> Result<()> {
    let (verb, repo_path) = parse_git_command(command)?;
    let (namespace, name) = config::parse_repo_path(repo_path)?;
    let user_id = auth::current_user_id(pool).await?;

    if !db::user_owns_repo(pool, user_id, namespace, name).await? {
        bail!("access denied to {namespace}/{name}");
    }

    let disk_path = config.repo_disk_path(namespace, name);
    if !disk_path.is_dir() {
        bail!("repository not found: {}", disk_path.display());
    }
    if !config::is_repo_path_inside_root(&config.repos_dir, &disk_path) {
        bail!("invalid repository path");
    }

    match verb {
        "git-receive-pack" => run_receive_pack(verb, &disk_path).await,
        _ => exec_git_helper(verb, &disk_path),
    }
}

async fn run_receive_pack(verb: &str, repo_path: &Path) -> Result<()> {
    let repo = repo_path
        .to_str()
        .context("repository path is not valid UTF-8")?;

    let status = Command::new(verb)
        .arg(repo)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("run {verb}"))?;

    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn parse_git_command(command: &str) -> Result<(&'static str, &str)> {
    let command = command.trim();
    let (verb, arg) = command
        .split_once(char::is_whitespace)
        .context("git command missing repository argument")?;
    let arg = arg
        .trim()
        .trim_matches('\'')
        .trim_matches('"');
    let verb = normalize_verb(verb)?;
    Ok((verb, arg))
}

fn normalize_verb(verb: &str) -> Result<&'static str> {
    let verb = verb.trim();
    let verb: Cow<'_, str> = if let Some(rest) = verb.strip_prefix("git ") {
        Cow::Owned(format!("git-{}", rest.replace(' ', "-")))
    } else {
        Cow::Borrowed(verb)
    };

    match verb.as_ref() {
        "git-upload-pack" => Ok("git-upload-pack"),
        "git-receive-pack" => Ok("git-receive-pack"),
        "git-upload-archive" => Ok("git-upload-archive"),
        _ => bail!("command not allowed: {verb}"),
    }
}

fn exec_git_helper(verb: &str, repo_path: &Path) -> Result<()> {
    let repo = repo_path
        .to_str()
        .context("repository path is not valid UTF-8")?;

    let mut cmd = Command::new(verb);
    cmd.arg(repo)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        return Err(err).with_context(|| format!("exec {verb}"));
    }

    #[cfg(not(unix))]
    {
        let status = cmd.status().context("run git helper")?;
        if status.success() {
            Ok(())
        } else {
            bail!("{verb} exited with {status}");
        }
    }
}

/// Install post-receive hooks on all repos in the database.
pub async fn install_hooks_all(config: &Config, pool: &SqlitePool) -> Result<()> {
    let rows = sqlx::query("SELECT namespace, name FROM repos")
        .fetch_all(pool)
        .await?;
    for row in rows {
        let namespace: String = row.get("namespace");
        let name: String = row.get("name");
        hook::install_post_receive_hook(config, &namespace, &name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_upload_pack() {
        let (verb, path) =
            parse_git_command("git-upload-pack 'public/demo.git'").unwrap();
        assert_eq!(verb, "git-upload-pack");
        assert_eq!(path, "public/demo.git");
    }
}
