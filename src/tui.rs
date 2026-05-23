use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use crate::db::DbPool;

use crate::auth;
use crate::config::{self, Config};
use crate::db::{self, Build, BuildStatus, Repo};
use crate::hook;
use crate::logo;

pub async fn run(config: &Config, pool: &DbPool) -> Result<()> {
    logo::show_welcome_logo();

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
            let status = db::latest_build_for_repo(pool, repo.id)
                .await?
                .map(|b| format!(" [{}]", b.status.as_str()))
                .unwrap_or_default();
            println!(
                "  {}. {}/{}  ({}){status}",
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
        browse_repo(config, pool, repo).await?;
    }

    Ok(())
}

async fn create_repo_flow(config: &Config, pool: &DbPool, user_id: i64) -> Result<()> {
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
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let status = Command::new("git")
        .args(["init", "--bare", &disk_path.to_string_lossy()])
        .status()
        .context("run git init --bare")?;
    if !status.success() {
        bail!("git init --bare failed");
    }

    db::create_repo(pool, user_id, NAMESPACE, &name).await?;
    hook::install_post_receive_hook(config, NAMESPACE, &name)?;

    println!();
    println!("Created {NAMESPACE}/{name}");
    println!("Clone with:");
    println!("  git clone {}", config.clone_url(NAMESPACE, &name));
    println!();
    println!("Start mjolnixd to build flakes on push: mjolnixd");
    Ok(())
}

async fn browse_repo(config: &Config, pool: &DbPool, repo: &Repo) -> Result<()> {
    let disk_path = config.repo_disk_path(&repo.namespace, &repo.name);
    loop {
        println!();
        println!("{}/{}", repo.namespace, repo.name);
        if let Some(build) = db::latest_build_for_repo(pool, repo.id).await? {
            print_build_summary(&build);
        } else {
            println!("  (no builds yet — push a branch with flake.nix)");
        }
        println!();
        println!("  1. List files on default branch");
        println!("  2. Build status (latest)");
        println!("  3. Build history");
        println!("  4. Back");
        print!("Choose: ");
        io::stdout().flush()?;

        match read_line()?.as_str() {
            "1" => list_repo_tree(&disk_path)?,
            "2" => show_latest_build(config, pool, repo).await?,
            "3" => show_build_history(config, pool, repo).await?,
            "4" => break,
            _ => println!("Invalid choice."),
        }
    }
    Ok(())
}

async fn show_latest_build(config: &Config, pool: &DbPool, repo: &Repo) -> Result<()> {
    let Some(build) = db::latest_build_for_repo(pool, repo.id).await? else {
        println!("No builds yet.");
        return Ok(());
    };
    print_build_detail(config, &build);
    Ok(())
}

async fn show_build_history(config: &Config, pool: &DbPool, repo: &Repo) -> Result<()> {
    let builds = db::list_builds_for_repo(pool, repo.id, 20).await?;
    if builds.is_empty() {
        println!("No builds yet.");
        return Ok(());
    }

    loop {
        println!();
        for (i, build) in builds.iter().enumerate() {
            println!(
                "  {}. #{} {} {} @ {} ({})",
                i + 1,
                build.id,
                short_rev(&build.rev),
                build.ref_name,
                build.created_at,
                build.status.as_str()
            );
        }
        println!("  b. Back");
        print!("Choose build to view: ");
        io::stdout().flush()?;

        let choice = read_line()?;
        if choice.eq_ignore_ascii_case("b") {
            break;
        }
        let Ok(index) = choice.parse::<usize>() else {
            println!("Invalid choice.");
            continue;
        };
        let Some(build) = builds.get(index.saturating_sub(1)) else {
            println!("Invalid choice.");
            continue;
        };
        print_build_detail(config, build);
        if let Some(path) = &build.log_path {
            print_log_tail(path, 50)?;
        }
    }
    Ok(())
}

fn print_build_summary(build: &Build) {
    println!(
        "  latest build: #{} {} — {}",
        build.id,
        short_rev(&build.rev),
        build.status.as_str()
    );
}

fn print_build_detail(config: &Config, build: &Build) {
    println!();
    println!("Build #{}", build.id);
    println!("  rev:      {}", build.rev);
    println!("  ref:      {}", build.ref_name);
    println!("  status:   {}", build.status.as_str());
    println!("  created:  {}", build.created_at);
    if let Some(t) = &build.started_at {
        println!("  started:  {t}");
    }
    if let Some(t) = &build.finished_at {
        println!("  finished: {t}");
    }
    if let Some(err) = &build.error_summary {
        println!("  error:    {err}");
    }
    if build.status == BuildStatus::Success {
        if let Some(paths) = &build.closure_paths {
            println!("  outputs:");
            for p in paths.iter().take(5) {
                println!("    {p}");
            }
            if paths.len() > 5 {
                println!("    … and {} more paths in closure", paths.len() - 5);
            }
        }
        if let Some(url) = &config.substituter_url {
            println!();
            println!("  Binary cache: {url}");
            println!("  Add to /etc/nix/nix.conf or ~/.config/nix/nix.conf:");
            println!("    extra-substituters = {url}");
            println!("    trusted-public-keys = <harmonia-public-key>");
            if let Some(paths) = &build.closure_paths
                && let Some(first) = paths.first()
            {
                println!("  Example: nix copy --from {url} {first}");
            }
        } else {
            println!();
            println!("  Set MJOLNIX_SUBSTITUTER_URL to show substituter hints.");
        }
    }
}

fn print_log_tail(log_path: &str, lines: usize) -> Result<()> {
    let content =
        std::fs::read_to_string(log_path).with_context(|| format!("read log {}", log_path))?;
    let tail: Vec<_> = content.lines().collect();
    let start = tail.len().saturating_sub(lines);
    println!();
    println!("--- log (last {lines} lines) ---");
    for line in &tail[start..] {
        println!("  {line}");
    }
    Ok(())
}

fn short_rev(rev: &str) -> &str {
    rev.get(..7).unwrap_or(rev)
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
    io::stdin().read_line(&mut buf).context("read stdin")?;
    Ok(buf.trim().to_string())
}
