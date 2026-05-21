mod auth;
mod config;
mod db;
mod git;
mod tui;

use std::io::{self, IsTerminal};

use anyhow::{Result, bail};

use crate::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = run().await {
        eprintln!("mjolnix: {err:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<()> {
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
