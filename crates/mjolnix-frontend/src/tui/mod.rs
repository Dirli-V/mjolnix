mod app;
mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use mjolnix_shared::config::Config;
use mjolnix_shared::db::DbPool;
use ratatui::DefaultTerminal;

use app::App;

pub async fn run(config: &Config, pool: &DbPool) -> Result<()> {
    let user_id = crate::auth::current_user_id(pool).await?;
    let mut app = App::new(config.clone(), user_id);
    app.reload_repos(pool).await?;
    if app.repos.is_empty() {
        app.screen = app::Screen::CreateRepo;
    }

    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app, config, pool).await;
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    config: &Config,
    pool: &DbPool,
) -> Result<()> {
    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        app.handle_key(key, config, pool).await?;
    }
    Ok(())
}
