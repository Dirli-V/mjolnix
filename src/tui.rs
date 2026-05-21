use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use sqlx::sqlite::SqlitePool;

use crate::auth;
use crate::config::{self, Config};
use crate::db::{self, Repo};

pub async fn run(config: &Config, pool: &SqlitePool) -> Result<()> {
    let user_id = auth::current_user_id(pool).await?;
    let mut repos = db::list_repos_for_user(pool, user_id).await?;

    println!("mjolnix — welcome");
    if repos.is_empty() {
        println!("You have no repositories yet.");
        println!();
        create_repo_flow(config, pool, user_id).await?;
        return Ok(());
    }

    loop {
        println!();
        println!("Your repositories:");
        for (i, repo) in repos.iter().enumerate() {
            println!(
                "  {}. {}/{}  ({})",
                i + 1,
                repo.namespace,
                repo.name,
                config.clone_url(&repo.namespace, &repo.name)
            );
        }
        println!();
        println!("  n. Create a new repository");
        println!("  q. Quit");
        print!("Choose: ");
        io::stdout().flush()?;

        let choice = read_line()?;
        if choice.eq_ignore_ascii_case("q") {
            break;
        }
        if choice.eq_ignore_ascii_case("n") {
            create_repo_flow(config, pool, user_id).await?;
            let updated = db::list_repos_for_user(pool, user_id).await?;
            repos.clear();
            repos.extend(updated);
            continue;
        }

        let Ok(index) = choice.parse::<usize>() else {
            println!("Invalid choice.");
            continue;
        };
        let Some(repo) = repos.get(index.saturating_sub(1)) else {
            println!("Invalid choice.");
            continue;
        };
        browse_repo(config, repo).await?;
    }

    Ok(())
}

async fn create_repo_flow(config: &Config, pool: &SqlitePool, user_id: i64) -> Result<()> {
    println!();
    print!("Repository name: ");
    io::stdout().flush()?;
    let name = read_line()?;
    config::validate_repo_name(&name)?;

    const NAMESPACE: &str = "public";

    let disk_path = config.repo_disk_path(NAMESPACE, &name);
    if disk_path.exists() {
        bail!("repository already exists at {}", disk_path.display());
    }

    if let Some(parent) = disk_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let status = Command::new("git")
        .args(["init", "--bare", &disk_path.to_string_lossy()])
        .status()
        .context("run git init --bare")?;
    if !status.success() {
        bail!("git init --bare failed");
    }

    db::create_repo(pool, user_id, NAMESPACE, &name).await?;

    println!();
    println!("Created {NAMESPACE}/{name}");
    println!("Clone with:");
    println!("  git clone {}", config.clone_url(NAMESPACE, &name));
    Ok(())
}

async fn browse_repo(config: &Config, repo: &Repo) -> Result<()> {
    let disk_path = config.repo_disk_path(&repo.namespace, &repo.name);
    loop {
        println!();
        println!("{}/{}", repo.namespace, repo.name);
        println!("  1. List files on default branch");
        println!("  2. Back");
        print!("Choose: ");
        io::stdout().flush()?;

        match read_line()?.as_str() {
            "1" => list_repo_tree(&disk_path)?,
            "2" => break,
            _ => println!("Invalid choice."),
        }
    }
    Ok(())
}

fn list_repo_tree(repo_path: &Path) -> Result<()> {
    let head = Command::new("git")
        .args([
            "--git-dir",
            &repo_path.to_string_lossy(),
            "rev-parse",
            "--verify",
            "HEAD",
        ])
        .output()
        .context("resolve HEAD")?;

    if !head.status.success() {
        println!("(empty repository — push commits with git first)");
        return Ok(());
    }

    let output = Command::new("git")
        .args([
            "--git-dir",
            &repo_path.to_string_lossy(),
            "ls-tree",
            "-r",
            "--name-only",
            "HEAD",
        ])
        .output()
        .context("git ls-tree")?;

    if !output.status.success() {
        bail!("git ls-tree failed");
    }

    let listing = String::from_utf8_lossy(&output.stdout);
    if listing.trim().is_empty() {
        println!("(no files at HEAD)");
    } else {
        for line in listing.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

fn read_line() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("read stdin")?;
    Ok(buf.trim().to_string())
}
