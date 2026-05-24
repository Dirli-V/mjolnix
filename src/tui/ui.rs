use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};

use super::app::{App, REPO_MENU_ITEMS, Screen, short_rev};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .split(area);

    draw_header(frame, chunks[0], app);
    draw_body(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" mjolnix │ user {} │ {} ", app.user_id, app.title());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(block, area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut hint = app.footer_hint().to_string();
    if let Some(msg) = &app.message {
        hint = format!("⚠ {msg}  ·  {hint}");
    }
    let footer = Paragraph::new(hint).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &App) {
    match app.screen {
        Screen::RepoList => draw_repo_list(frame, area, app),
        Screen::CreateRepo => draw_create_repo(frame, area, app),
        Screen::RepoMenu => draw_repo_menu(frame, area, app),
        Screen::BuildHistory => draw_build_history(frame, area, app),
        Screen::ScrollView => draw_scroll_view(frame, area, app),
    }
}

fn draw_repo_list(frame: &mut Frame, area: Rect, app: &App) {
    if app.repos.is_empty() {
        let text = vec![
            Line::from(""),
            Line::from("You have no repositories yet."),
            Line::from(""),
            Line::from("Press n to create your first repository."),
        ];
        let p = Paragraph::new(text)
            .block(panel_block("Welcome"))
            .alignment(Alignment::Center);
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem> = app
        .repos
        .iter()
        .enumerate()
        .map(|(i, repo)| {
            let status = app.repo_statuses.get(i).map(String::as_str).unwrap_or("");
            let clone_url = app.config.clone_url(&repo.namespace, &repo.name);
            let line = format!("{}/{}  {clone_url}{status}", repo.namespace, repo.name);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(panel_block("Your repositories"))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = app.repo_list;
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_create_repo(frame: &mut Frame, area: Rect, app: &App) {
    let name = if app.create_name.is_empty() {
        "_".repeat(20)
    } else {
        app.create_name.clone()
    };
    let text = vec![
        Line::from(""),
        Line::from("Repository name (namespace: public)"),
        Line::from(""),
        Line::from(format!("  {name}")),
    ];
    let p = Paragraph::new(text).block(panel_block("Create repository"));
    frame.render_widget(p, area);
}

fn draw_repo_menu(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(6),
        Constraint::Length(8),
    ])
    .margin(1)
    .split(area);

    let summary = app.repo_build_summary();
    let summary_p = Paragraph::new(summary).block(Block::default().borders(Borders::LEFT));
    frame.render_widget(summary_p, chunks[0]);

    let items: Vec<ListItem> = REPO_MENU_ITEMS.iter().map(|s| ListItem::new(*s)).collect();
    let list = List::new(items)
        .block(panel_block("Actions"))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut menu_state = app.repo_menu;
    frame.render_stateful_widget(list, chunks[1], &mut menu_state);

    if !app.cache_lines.is_empty() {
        let cache_text: Vec<Line> = app
            .cache_lines
            .iter()
            .map(|l| Line::from(l.as_str()))
            .collect();
        let cache_p = Paragraph::new(cache_text)
            .block(panel_block("Binary cache"))
            .wrap(Wrap { trim: false });
        frame.render_widget(cache_p, chunks[2]);
    }
}

fn draw_build_history(frame: &mut Frame, area: Rect, app: &App) {
    if app.builds.is_empty() {
        let p = Paragraph::new("No builds yet.")
            .block(panel_block("Builds"))
            .alignment(Alignment::Center);
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem> = app
        .builds
        .iter()
        .map(|build| {
            ListItem::new(format!(
                "#{} {} {} @ {} ({})",
                build.id,
                short_rev(&build.rev),
                build.ref_name,
                build.created_at,
                build.status.as_str()
            ))
        })
        .collect();

    let list = List::new(items)
        .block(panel_block("Recent builds"))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = app.build_list;
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_scroll_view(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel_block(&app.scroll.title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = app.scroll.lines.join("\n");
    let paragraph = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll.offset as u16, 0));

    frame.render_widget(paragraph, inner);

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));
    let mut scroll_state = ScrollbarState::new(app.scroll.lines.len()).position(app.scroll.offset);
    frame.render_stateful_widget(
        scrollbar,
        inner.inner(Margin {
            vertical: 0,
            horizontal: 1,
        }),
        &mut scroll_state,
    );
}

fn panel_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(format!(" {title} "))
        .title_style(Style::default().fg(Color::Cyan))
}
