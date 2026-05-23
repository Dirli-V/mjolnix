use std::env;
use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail};
use mjolnix::config::Config;
use mjolnix::{db, git, hook, tui};

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = run().await {
        eprintln!("mjolnix: {err:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hook-post-receive") => {
            let namespace = args.get(2).context("missing namespace")?;
            let name = args.get(3).context("missing name")?;
            let old = args.get(4).context("missing old")?;
            let new = args.get(5).context("missing new")?;
            let ref_name = args.get(6).context("missing ref")?;
            return run_hook_post_receive(namespace, name, old, new, ref_name).await;
        }
        Some("install-hooks") => return run_install_hooks().await,
        Some("help" | "--help" | "-h") => {
            print_help();
            return Ok(());
        }
        _ => {}
    }

    run_default().await
}

async fn run_default() -> Result<()> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;

    if let Some(command) = git::remote_git_command() {
        return git::run(&config, &pool, &command).await;
    }

    if io::stdin().is_terminal() {
        return tui::run(&config, &pool).await;
    }

    bail!("nothing to do: expected an SSH git command or an interactive terminal");
}

async fn run_hook_post_receive(
    namespace: &str,
    name: &str,
    old: &str,
    new: &str,
    ref_name: &str,
) -> Result<()> {
    let config = Config::from_env()?;
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    hook::hook_post_receive(&config, &pool, namespace, name, old, new, ref_name).await
}

async fn run_install_hooks() -> Result<()> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    git::install_hooks_all(&config, &pool).await?;
    println!("installed post-receive hooks on all repositories");
    Ok(())
}

fn print_help() {
    println!(
        r#"mjolnix — git hosting with Nix builds

Usage:
  mjolnix                         interactive TUI (SSH) or git wrapper
  mjolnix hook-post-receive ...   post-receive hook entry (internal)
  mjolnix install-hooks           install hooks on existing repositories
  mjolnixd                        build daemon (separate binary)

Environment:
  MJOLNIX_DATABASE_URL            PostgreSQL connection URL (required)
  MJOLNIX_DATA_DIR, MJOLNIX_HOST, MJOLNIX_KEY_FINGERPRINT, MJOLNIX_USER_ID
  MJOLNIX_SUBSTITUTER_URL         binary cache URL shown after successful builds
"#
    );
}
