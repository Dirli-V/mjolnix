use anyhow::Result;
use mjolnix::config::Config;
use mjolnix::daemon;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = daemon::run(Config::from_env()?).await {
        eprintln!("mjolnixd: {err:#}");
        std::process::exit(1);
    }
    Ok(())
}
