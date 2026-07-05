use std::env;
use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail};
use mjolnix_shared::config::Config;
use mjolnix_shared::db;

mod auth;
mod git;
mod hook;
mod tui;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = run().await {
        eprintln!("mjolnix-frontend: {err:#}");
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
        Some("help" | "--help" | "-h") => {
            print_help();
            return Ok(());
        }
        _ => {}
    }

    let config = Config::from_env()?;
    config.ensure_dirs()?;
    let pool = db::connect(&config).await?;

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
    hook::hook_post_receive(&config, &pool, namespace, name, old, new, ref_name).await
}

fn print_help() {
    println!(
        r#"mjolnix-frontend

Usage:
  mjolnix-frontend               interactive TUI (SSH) or git wrapper
  mjolnix-frontend hook-post-receive ...   post-receive hook entry (internal)

Environment:
  MJOLNIX_DATABASE_URL, MJOLNIX_DATA_DIR, MJOLNIX_HOST, MJOLNIX_USER_ID
"#
    );
}
