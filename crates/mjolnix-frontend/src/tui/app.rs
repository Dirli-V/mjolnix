use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mjolnix_shared::config::{self, Config};
use mjolnix_shared::db::{self, Build, BuildStatus, DbPool, Repo};
use mjolnix_shared::signing;
use mjolnix_shared::store::{self, RepoStore};
use ratatui::widgets::ListState;

use crate::hook;

pub const REPO_MENU_ITEMS: &[&str] = &[
    "List files on default branch",
    "Build status (latest)",
    "Build history",
];

pub struct App {
    pub config: Config,
    pub user_id: i64,
    pub repos: Vec<Repo>,
    pub repo_statuses: Vec<String>,
    pub repo_list: ListState,
    pub screen: Screen,
    pub scroll_return: Screen,
    pub quit: bool,
    pub message: Option<String>,
    pub create_name: String,
    pub repo_menu: ListState,
    pub current_repo: Option<Repo>,
    pub latest_build: Option<Build>,
    pub cache_lines: Vec<String>,
    pub builds: Vec<Build>,
    pub build_list: ListState,
    pub scroll: ScrollView,
}

pub struct ScrollView {
    pub title: String,
    pub lines: Vec<String>,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    RepoList,
    CreateRepo,
    RepoMenu,
    BuildHistory,
    ScrollView,
}

impl App {
    pub fn new(config: Config, user_id: i64) -> Self {
        let mut repo_list = ListState::default();
        repo_list.select(Some(0));
        Self {
            config,
            user_id,
            repos: Vec::new(),
            repo_statuses: Vec::new(),
            repo_list,
            screen: Screen::RepoList,
            scroll_return: Screen::RepoList,
            quit: false,
            message: None,
            create_name: String::new(),
            repo_menu: ListState::default(),
            current_repo: None,
            latest_build: None,
            cache_lines: Vec::new(),
            builds: Vec::new(),
            build_list: ListState::default(),
            scroll: ScrollView {
                title: String::new(),
                lines: Vec::new(),
                offset: 0,
            },
        }
    }

    pub async fn reload_repos(&mut self, pool: &DbPool) -> Result<()> {
        self.repos = db::list_repos_for_user(pool, self.user_id).await?;
        self.repo_statuses.clear();
        for repo in &self.repos {
            let tag = db::latest_build_for_repo(pool, repo.id)
                .await?
                .map(|b| format!(" [{}]", b.status.as_str()))
                .unwrap_or_default();
            self.repo_statuses.push(tag);
        }
        if self.repos.is_empty() {
            self.repo_list.select(None);
        } else if self.repo_list.selected().is_none()
            || self.repo_list.selected() >= Some(self.repos.len())
        {
            self.repo_list.select(Some(0));
        }
        Ok(())
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::RepoList => "Repositories".into(),
            Screen::CreateRepo => "New repository".into(),
            Screen::RepoMenu => self
                .current_repo
                .as_ref()
                .map(|r| format!("{}/{}", r.namespace, r.name))
                .unwrap_or_else(|| "Repository".into()),
            Screen::BuildHistory => "Build history".into(),
            Screen::ScrollView => self.scroll.title.clone(),
        }
    }

    pub fn footer_hint(&self) -> &'static str {
        match self.screen {
            Screen::RepoList => {
                if self.repos.is_empty() {
                    "n new repository  .  q quit"
                } else {
                    "up/down move  .  Enter open  .  n new  .  q quit"
                }
            }
            Screen::CreateRepo => "Enter create  .  Esc cancel  .  type name",
            Screen::RepoMenu => "up/down move  .  Enter select  .  Esc back  .  q quit",
            Screen::BuildHistory => "up/down move  .  Enter details  .  Esc back",
            Screen::ScrollView => "up/down/PgUp/PgDn scroll  .  Esc back",
        }
    }

    pub async fn handle_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        pool: &DbPool,
    ) -> Result<()> {
        self.message = None;
        match self.screen {
            Screen::RepoList => self.key_repo_list(key, pool).await?,
            Screen::CreateRepo => self.key_create_repo(key, config, pool).await?,
            Screen::RepoMenu => self.key_repo_menu(key, config, pool).await?,
            Screen::BuildHistory => self.key_build_history(key, config, pool).await?,
            Screen::ScrollView => self.key_scroll(key),
        }
        Ok(())
    }

    async fn key_repo_list(&mut self, key: KeyEvent, pool: &DbPool) -> Result<()> {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.create_name.clear();
                self.screen = Screen::CreateRepo;
            }
            KeyCode::Down | KeyCode::Char('j') => self.select_next_repo(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev_repo(),
            KeyCode::Enter => {
                if self.repos.is_empty() {
                    self.create_name.clear();
                    self.screen = Screen::CreateRepo;
                } else if let Some(repo) = self.selected_repo().cloned() {
                    self.open_repo(pool, &repo).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn key_create_repo(
        &mut self,
        key: KeyEvent,
        config: &Config,
        pool: &DbPool,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.screen = Screen::RepoList,
            KeyCode::Enter => match self.submit_create_repo(config, pool).await {
                Ok(()) => self.reload_repos(pool).await?,
                Err(e) => self.message = Some(format!("{e:#}")),
            },
            KeyCode::Backspace => {
                self.create_name.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.create_name.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    async fn key_repo_menu(&mut self, key: KeyEvent, config: &Config, pool: &DbPool) -> Result<()> {
        let repo = match self.current_repo.clone() {
            Some(r) => r,
            None => {
                self.screen = Screen::RepoList;
                return Ok(());
            }
        };
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::RepoList;
                self.current_repo = None;
                self.latest_build = None;
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.select_next_menu(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev_menu(),
            KeyCode::Enter => {
                let sel = self.repo_menu.selected().unwrap_or(0);
                match sel {
                    0 => {
                        let disk = config.repo_disk_path(&repo.namespace, &repo.name);
                        let lines = list_repo_tree_lines(&disk)?;
                        self.show_scroll(Screen::RepoMenu, "Files at HEAD", lines);
                    }
                    1 => {
                        let lines = if let Some(build) = &self.latest_build {
                            build_detail_lines(&self.config, &repo, build).await?
                        } else {
                            vec!["No builds yet.".into()]
                        };
                        self.show_scroll(Screen::RepoMenu, "Latest build", lines);
                    }
                    2 => {
                        self.builds = db::list_builds_for_repo(pool, repo.id, 20).await?;
                        self.build_list.select(if self.builds.is_empty() {
                            None
                        } else {
                            Some(0)
                        });
                        self.screen = Screen::BuildHistory;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn key_build_history(&mut self, key: KeyEvent, config: &Config, _pool: &DbPool) -> Result<()> {
        let repo = match self.current_repo.as_ref() {
            Some(r) => r,
            None => return Ok(()),
        };
        match key.code {
            KeyCode::Esc => self.screen = Screen::RepoMenu,
            KeyCode::Down | KeyCode::Char('j') => self.select_next_build(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev_build(),
            KeyCode::Enter => {
                if let Some(build) = self.selected_build().cloned() {
                    let mut lines = build_detail_lines(config, repo, &build).await?;
                    if let Some(path) = &build.log_path {
                        lines.push(String::new());
                        lines.push("--- log (last 50 lines) ---".into());
                        lines.extend(log_tail_lines(path, 50)?);
                    }
                    self.show_scroll(Screen::BuildHistory, format!("Build #{}", build.id), lines);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn key_scroll(&mut self, key: KeyEvent) {
        let max = self.scroll.lines.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.screen = self.scroll_return,
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll.offset = (self.scroll.offset + 1).min(max)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll.offset = self.scroll.offset.saturating_sub(1)
            }
            KeyCode::PageDown => self.scroll.offset = (self.scroll.offset + 10).min(max),
            KeyCode::PageUp => self.scroll.offset = self.scroll.offset.saturating_sub(10),
            KeyCode::Home => self.scroll.offset = 0,
            KeyCode::End => self.scroll.offset = max,
            _ => {}
        }
    }

    fn show_scroll(&mut self, return_to: Screen, title: impl Into<String>, lines: Vec<String>) {
        self.scroll_return = return_to;
        self.scroll.title = title.into();
        self.scroll.lines = lines;
        self.scroll.offset = 0;
        self.screen = Screen::ScrollView;
    }

    async fn open_repo(&mut self, pool: &DbPool, repo: &Repo) -> Result<()> {
        self.current_repo = Some(repo.clone());
        self.latest_build = db::latest_build_for_repo(pool, repo.id).await?;
        self.cache_lines = repo_cache_hint_lines(&repo_store_for(&self.config, repo).await?);
        self.repo_menu.select(Some(0));
        self.screen = Screen::RepoMenu;
        Ok(())
    }

    fn selected_repo(&self) -> Option<&Repo> {
        let i = self.repo_list.selected()?;
        self.repos.get(i)
    }
    fn selected_build(&self) -> Option<&Build> {
        let i = self.build_list.selected()?;
        self.builds.get(i)
    }

    fn select_next_repo(&mut self) {
        if self.repos.is_empty() {
            return;
        }
        let next = self
            .repo_list
            .selected()
            .map(|i| (i + 1) % self.repos.len())
            .unwrap_or(0);
        self.repo_list.select(Some(next));
    }

    fn select_prev_repo(&mut self) {
        if self.repos.is_empty() {
            return;
        }
        let len = self.repos.len();
        let prev = self
            .repo_list
            .selected()
            .map(|i| (i + len - 1) % len)
            .unwrap_or(0);
        self.repo_list.select(Some(prev));
    }

    fn select_next_menu(&mut self) {
        let next = self
            .repo_menu
            .selected()
            .map(|i| (i + 1) % REPO_MENU_ITEMS.len())
            .unwrap_or(0);
        self.repo_menu.select(Some(next));
    }

    fn select_prev_menu(&mut self) {
        let len = REPO_MENU_ITEMS.len();
        let prev = self
            .repo_menu
            .selected()
            .map(|i| (i + len - 1) % len)
            .unwrap_or(0);
        self.repo_menu.select(Some(prev));
    }

    fn select_next_build(&mut self) {
        if self.builds.is_empty() {
            return;
        }
        let next = self
            .build_list
            .selected()
            .map(|i| (i + 1) % self.builds.len())
            .unwrap_or(0);
        self.build_list.select(Some(next));
    }

    fn select_prev_build(&mut self) {
        if self.builds.is_empty() {
            return;
        }
        let len = self.builds.len();
        let prev = self
            .build_list
            .selected()
            .map(|i| (i + len - 1) % len)
            .unwrap_or(0);
        self.build_list.select(Some(prev));
    }

    pub fn repo_build_summary(&self) -> String {
        match &self.latest_build {
            Some(b) => format!("Latest build #{} - {}", b.id, b.status.as_str()),
            None => "No builds yet - push a branch with flake.nix".into(),
        }
    }

    async fn submit_create_repo(&mut self, config: &Config, pool: &DbPool) -> Result<()> {
        let name = self.create_name.trim().to_string();
        if name.is_empty() {
            bail!("repository name cannot be empty");
        }
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
        let repo_id = db::create_repo(pool, config, self.user_id, NAMESPACE, &name).await?;
        hook::install_post_receive_hook(config, NAMESPACE, &name)?;

        let mut lines = vec![
            format!("Created {NAMESPACE}/{name}"),
            format!("Clone: git clone {}", config.clone_url(NAMESPACE, &name)),
        ];
        let repo = db::Repo {
            id: repo_id,
            namespace: NAMESPACE.into(),
            name: name.clone(),
        };
        lines.extend(repo_cache_hint_lines(&repo_store_for(config, &repo).await?));
        lines.push("Start worker for builds: mjolnix-worker".into());
        self.show_scroll(Screen::RepoList, "Repository created", lines);
        Ok(())
    }
}

pub fn short_rev(rev: &str) -> &str {
    rev.get(..7).unwrap_or(rev)
}

fn list_repo_tree_lines(repo_path: &Path) -> Result<Vec<String>> {
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
        return Ok(vec![
            "(empty repository - push commits with git first)".into(),
        ]);
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
        return Ok(vec!["(no files at HEAD)".into()]);
    }
    Ok(listing.lines().map(|l| format!("  {l}")).collect())
}

fn log_tail_lines(log_path: &str, lines: usize) -> Result<Vec<String>> {
    let content =
        std::fs::read_to_string(log_path).with_context(|| format!("read log {}", log_path))?;
    let tail: Vec<_> = content.lines().collect();
    let start = tail.len().saturating_sub(lines);
    Ok(tail[start..].iter().map(|l| format!("  {l}")).collect())
}

async fn repo_store_for(config: &Config, repo: &Repo) -> Result<RepoStore> {
    let (uid, gid) = store::process_uid_gid();
    let cache_public_key = signing::try_load_secret_key(&config.cache_sign_key_path)
        .await?
        .map(|key| key.public_key_line);
    Ok(store::repo_store(config, repo, uid, gid, cache_public_key))
}

fn repo_cache_hint_lines(repo_store: &RepoStore) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("Binary cache: {}", repo_store.substituter_url));
    lines.push("Add to /etc/nix/nix.conf or ~/.config/nix/nix.conf:".into());
    lines.push(format!(
        "  extra-substituters = {}",
        repo_store.substituter_url
    ));
    match &repo_store.cache_public_key {
        Some(pk) => lines.push(format!("  trusted-public-keys = {pk}")),
        None => lines.push(
            "  trusted-public-keys = <unavailable - enable cache and ensure nix is installed>"
                .into(),
        ),
    }
    lines
}

async fn build_detail_lines(config: &Config, repo: &Repo, build: &Build) -> Result<Vec<String>> {
    let mut lines = vec![
        format!("Build #{}", build.id),
        format!("  rev:      {}", build.rev),
        format!("  ref:      {}", build.ref_name),
        format!("  status:   {}", build.status.as_str()),
        format!("  created:  {}", build.created_at),
    ];
    if let Some(t) = &build.started_at {
        lines.push(format!("  started:  {t}"));
    }
    if let Some(t) = &build.finished_at {
        lines.push(format!("  finished: {t}"));
    }
    if let Some(err) = &build.error_summary {
        lines.push(format!("  error:    {err}"));
    }
    if build.status == BuildStatus::Success {
        if let Some(paths) = &build.closure_paths {
            lines.push("  outputs:".into());
            for p in paths.iter().take(5) {
                lines.push(format!("    {p}"));
            }
            if paths.len() > 5 {
                lines.push(format!(
                    "    ... and {} more paths in closure",
                    paths.len() - 5
                ));
            }
        }
        let repo_store = repo_store_for(config, repo).await?;
        lines.push(String::new());
        lines.extend(repo_cache_hint_lines(&repo_store));
        if let Some(paths) = &build.closure_paths
            && let Some(first) = paths.first()
        {
            lines.push(format!(
                "  Example: nix copy --from {} {first}",
                repo_store.substituter_url
            ));
        }
    }
    Ok(lines)
}
