//! Interactive manager for contexts and repos, lazygit-style.
//!
//! One thread owns all state: the event loop below receives key input,
//! timer ticks, and worker results over a channel and mutates the app in
//! response. Anything subprocess-heavy (creates, archives, status probes)
//! runs on worker threads that only ever report back as events.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell as TableCell, Clear, Paragraph, Row, Table, TableState, Wrap,
};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::config::Config;
use crate::contexts::{self, Context};
use crate::errors::Result as CtxResult;
use crate::git::new_command;
use crate::multiplexer::Multiplexer;
use crate::{forge, repos, status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Open {
        name: String,
    },
    New {
        repo: String,
        name: String,
        base: Option<String>,
    },
}

const SPINNER_FRAMES: [char; 4] = ['|', '/', '-', '\\'];

const STATUS_POLL_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Contexts,
    Repos,
    Archived,
}

impl Panel {
    const ALL: [Panel; 3] = [Panel::Contexts, Panel::Repos, Panel::Archived];

    fn title(self) -> &'static str {
        match self {
            Panel::Contexts => "[1] Contexts",
            Panel::Repos => "[2] Repos",
            Panel::Archived => "[3] Archived",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Panel::Contexts => "contexts",
            Panel::Repos => "repos",
            Panel::Archived => "archived",
        }
    }

    fn cycle(self, step: isize) -> Panel {
        let index = Panel::ALL
            .iter()
            .position(|p| *p == self)
            .expect("panel listed") as isize;
        let next = (index + step).rem_euclid(Panel::ALL.len() as isize);
        Panel::ALL[next as usize]
    }
}

/// One table cell: text plus the status vocabulary's style word, if any.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellValue {
    text: String,
    style: Option<&'static str>,
}

impl CellValue {
    fn plain(text: impl Into<String>) -> CellValue {
        CellValue {
            text: text.into(),
            style: None,
        }
    }

    fn styled(text: impl Into<String>, style: Option<&'static str>) -> CellValue {
        CellValue {
            text: text.into(),
            style,
        }
    }
}

#[derive(Debug, Clone)]
struct TableRow {
    key: String,
    cells: Vec<CellValue>,
}

/// A panel's table: keyed rows plus a cursor, ratatui-agnostic for tests.
struct PanelTable {
    headers: Vec<String>,
    rows: Vec<TableRow>,
    cursor: usize,
    view: TableState,
}

impl PanelTable {
    fn new(headers: Vec<String>) -> PanelTable {
        PanelTable {
            headers,
            rows: Vec::new(),
            cursor: 0,
            view: TableState::default(),
        }
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.cursor = 0;
    }

    fn add_row(&mut self, key: impl Into<String>, cells: Vec<CellValue>) {
        self.rows.push(TableRow {
            key: key.into(),
            cells,
        });
    }

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn move_cursor(&mut self, row: isize) {
        if self.rows.is_empty() {
            self.cursor = 0;
            return;
        }
        self.cursor = row.clamp(0, self.rows.len() as isize - 1) as usize;
    }

    fn selected_key(&self) -> Option<&str> {
        self.rows.get(self.cursor).map(|row| row.key.as_str())
    }

    fn update_cell(&mut self, key: &str, column: usize, cell: CellValue) {
        // The row may have been deleted or archived since the fetch started.
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key)
            && let Some(slot) = row.cells.get_mut(column)
        {
            *slot = cell;
        }
    }

    fn cursor_to_key(&mut self, key: &str) {
        if let Some(index) = self.rows.iter().position(|row| row.key == key) {
            self.cursor = index;
        }
    }
}

enum PromptKind {
    NewName { repo: String },
    NewBase { repo: String, name: String },
    AddRepo,
}

enum ConfirmKind {
    DeleteLive(Context),
    DeleteArchived(Context),
    RemoveRepo(String),
    EmptyArchive,
}

enum Modal {
    Prompt {
        title: String,
        placeholder: &'static str,
        input: Input,
        // A pre-filled value is replaced by the first keystroke, like a
        // selected-on-focus input.
        replace_on_type: bool,
        kind: PromptKind,
    },
    Alert {
        message: String,
    },
    Help {
        panel: Panel,
    },
    Confirm {
        message: String,
        confirm_label: &'static str,
        selected: usize,
        kind: ConfirmKind,
    },
}

struct Filter {
    target: Panel,
    query: Input,
    rows: Vec<TableRow>,
}

/// A worker's report back to the event loop.
#[derive(Debug, Default)]
pub struct WorkerDone {
    reload: bool,
    finish_busy: bool,
    alert: Option<String>,
    exit: bool,
    /// The worker's last report; decrements the outstanding-worker count.
    finished: bool,
}

pub enum Event {
    Key(KeyEvent),
    Mouse(ratatui::crossterm::event::MouseEvent),
    Tick,
    Cell {
        key: String,
        column: usize,
        cell: CellValue,
    },
    ColumnDone(usize),
    Worker(WorkerDone),
    Redraw,
}

pub struct CtxTui {
    cfg: Config,
    mux: Arc<dyn Multiplexer>,
    exit_on_open: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    panel: Panel,
    contexts: PanelTable,
    repos: PanelTable,
    archived: PanelTable,
    busy: HashSet<Panel>,
    spinner_frame: usize,
    fetching: HashSet<usize>,
    poll_at: Vec<Instant>,
    filter: Option<Filter>,
    modal: Option<Modal>,
    workers: usize,
    outcome: Option<Request>,
    quit: bool,
    // Panel rectangles from the last render, for mouse hit-testing.
    areas: HashMap<Panel, Rect>,
    button_areas: Vec<Rect>,
}

fn spawn_worker<F: FnOnce() + Send + 'static>(f: F) {
    #[cfg(test)]
    let f = crate::testutil::propagate_env(f);
    std::thread::spawn(f);
}

impl CtxTui {
    pub fn new(cfg: Config, mux: Arc<dyn Multiplexer>, exit_on_open: bool) -> CtxTui {
        let (tx, rx) = std::sync::mpsc::channel();
        let status_names: Vec<String> = cfg.status.iter().map(|s| s.name.to_uppercase()).collect();
        let mut contexts_headers = vec![
            "NAME".to_string(),
            "REPO".to_string(),
            "BRANCH".to_string(),
            "STATUS".to_string(),
        ];
        contexts_headers.extend(status_names);
        let now = Instant::now();
        let intervals = 1 + cfg.status.len();
        CtxTui {
            cfg,
            mux,
            exit_on_open,
            tx,
            rx,
            panel: Panel::Contexts,
            contexts: PanelTable::new(contexts_headers),
            repos: PanelTable::new(vec!["NAME".to_string(), "URL".to_string()]),
            archived: PanelTable::new(vec![
                "NAME".to_string(),
                "REPO".to_string(),
                "BRANCH".to_string(),
            ]),
            busy: HashSet::new(),
            spinner_frame: 0,
            fetching: HashSet::new(),
            poll_at: vec![now; intervals],
            filter: None,
            modal: None,
            workers: 0,
            outcome: None,
            quit: false,
            areas: HashMap::new(),
            button_areas: Vec::new(),
        }
    }

    /// Each status column's poll cadence: its provider's refresh interval.
    ///
    /// The STATUS column and columns without an interval ride the base tick;
    /// nothing polls faster than it.
    fn poll_intervals(&self) -> Vec<Duration> {
        let mut intervals = vec![Duration::from_secs_f64(STATUS_POLL_SECONDS)];
        intervals.extend(self.cfg.status.iter().map(|col| {
            Duration::from_secs_f64(status::refresh_interval(col).max(STATUS_POLL_SECONDS))
        }));
        intervals
    }

    /// Startup work: populate the panels and sweep interrupted deletions.
    pub fn mount(&mut self) {
        self.reload();
        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        self.workers += 1;
        spawn_worker(move || {
            // Finish any context deletions a previous run left half-done.
            contexts::sweep_deleting(&cfg);
            let _ = tx.send(Event::Worker(WorkerDone {
                finished: true,
                ..WorkerDone::default()
            }));
        });
        let intervals = self.poll_intervals();
        let now = Instant::now();
        self.poll_at = intervals.iter().map(|interval| now + *interval).collect();
    }

    /// Repaint the panels from what is cheap to read; statuses fill in after.
    ///
    /// A status provider may take seconds per context (the GitHub built-ins
    /// shell out to `gh`), which is more than the panels can wait for and far
    /// more than the interface can stop responding for.
    fn reload(&mut self) {
        if let Some(filter) = self.filter.take() {
            // A reload repopulates every panel, so the snapshot is stale.
            self.panel = filter.target;
        }
        let blanks = 1 + self.cfg.status.len();
        self.contexts.clear();
        let mut ctxs = contexts::list_contexts(&self.cfg);
        // Pin the attached context on top: recency is keyed on git activity,
        // so a busy background session often outranks the one being viewed.
        let current = ctxs.iter().position(|c| self.mux.is_current(c));
        if let Some(index) = current {
            let ctx = ctxs.remove(index);
            ctxs.insert(0, ctx);
        }
        for (index, ctx) in ctxs.iter().enumerate() {
            let name_style = (current.is_some() && index == 0).then_some("bold bright_green");
            let mut cells = vec![
                CellValue::styled(&ctx.name, name_style),
                CellValue::plain(&ctx.repo),
                CellValue::plain(contexts::current_branch(ctx)),
            ];
            cells.extend(std::iter::repeat_with(CellValue::default).take(blanks));
            self.contexts.add_row(&ctx.name, cells);
        }
        // Land the cursor on the most recent other context: the common reason
        // to open the TUI is switching away, not reopening the same session.
        if current.is_some() && self.contexts.row_count() > 1 {
            self.contexts.move_cursor(1);
        }
        self.repos.clear();
        let default = repos::default_repo(&self.cfg);
        let mut names = repos::repo_names(&self.cfg);
        names.sort_by_key(|name| (Some(name) != default.as_ref(), name.clone()));
        for name in names {
            let label = if Some(&name) == default.as_ref() {
                format!("{name} *")
            } else {
                name.clone()
            };
            let url = repos::repo_url(&self.cfg, &name).unwrap_or_default();
            self.repos
                .add_row(&name, vec![CellValue::plain(label), CellValue::plain(url)]);
        }
        self.archived.clear();
        for ctx in contexts::list_archived(&self.cfg) {
            self.archived.add_row(
                &ctx.name,
                vec![
                    CellValue::plain(&ctx.name),
                    CellValue::plain(&ctx.repo),
                    CellValue::plain(contexts::current_branch(&ctx)),
                ],
            );
        }
        self.refresh_statuses();
    }

    fn refresh_statuses(&mut self) {
        for index in 0..=self.cfg.status.len() {
            self.refresh_column(index);
        }
    }

    /// Fetch one column's cells concurrently, painting each as it lands.
    fn refresh_column(&mut self, index: usize) {
        if self.fetching.contains(&index) {
            return;
        }
        self.fetching.insert(index);
        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        spawn_worker(move || {
            let ctxs = contexts::list_contexts(&cfg);
            std::thread::scope(|scope| {
                for ctx in &ctxs {
                    let tx = tx.clone();
                    let cfg = &cfg;
                    scope.spawn(move || {
                        let cell = fetch_cell(cfg, ctx, index);
                        let _ = tx.send(Event::Cell {
                            key: ctx.name.clone(),
                            column: index,
                            cell,
                        });
                    });
                }
            });
            let _ = tx.send(Event::ColumnDone(index));
        });
    }

    /// Keep one status column live without a full (cursor-resetting) reload.
    fn poll_column(&mut self, index: usize) {
        if self.busy.is_empty() {
            self.refresh_column(index);
        }
    }

    fn start_busy(&mut self, panel: Panel) {
        self.busy.insert(panel);
    }

    fn finish_busy(&mut self) {
        self.busy.clear();
    }

    fn table(&self, panel: Panel) -> &PanelTable {
        match panel {
            Panel::Contexts => &self.contexts,
            Panel::Repos => &self.repos,
            Panel::Archived => &self.archived,
        }
    }

    fn table_mut(&mut self, panel: Panel) -> &mut PanelTable {
        match panel {
            Panel::Contexts => &mut self.contexts,
            Panel::Repos => &mut self.repos,
            Panel::Archived => &mut self.archived,
        }
    }

    /// Centrally disable mutating actions while a worker runs or a popup is open.
    fn allow_mutation(&self) -> bool {
        self.busy.is_empty() && self.modal.is_none()
    }

    /// Resolve the cursor's context from disk; reloads and yields None if stale.
    fn selected_context(&mut self) -> Option<Context> {
        let key = self.contexts.selected_key()?.to_string();
        match contexts::find_context(&self.cfg, &key) {
            Ok(ctx) => Some(ctx),
            Err(_) => {
                // The row went stale, e.g. the context was deleted externally.
                self.reload();
                None
            }
        }
    }

    /// Resolve the archived panel's cursor from disk; reloads and yields None if stale.
    fn selected_archived(&mut self) -> Option<Context> {
        let key = self.archived.selected_key()?.to_string();
        match contexts::find_archived(&self.cfg, &key) {
            Ok(ctx) => Some(ctx),
            Err(_) => {
                self.reload();
                None
            }
        }
    }

    fn alert(&mut self, message: impl Into<String>) {
        self.modal = Some(Modal::Alert {
            message: message.into(),
        });
    }

    // ------------------------------------------------------------------
    // Event handling

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Tick => self.handle_tick(),
            Event::Cell { key, column, cell } => {
                self.contexts.update_cell(&key, 3 + column, cell);
            }
            Event::ColumnDone(index) => {
                self.fetching.remove(&index);
            }
            Event::Worker(done) => self.handle_worker(done),
            Event::Redraw => {}
        }
    }

    fn handle_worker(&mut self, done: WorkerDone) {
        if done.finished {
            self.workers = self.workers.saturating_sub(1);
        }
        if done.reload {
            self.reload();
        }
        if done.finish_busy {
            self.finish_busy();
        }
        if let Some(message) = done.alert {
            self.alert(message);
        }
        if done.exit {
            self.quit = true;
        }
    }

    fn handle_tick(&mut self) {
        if !self.busy.is_empty() {
            self.spinner_frame += 1;
        }
        let intervals = self.poll_intervals();
        let now = Instant::now();
        for (index, interval) in intervals.iter().enumerate() {
            if now >= self.poll_at[index] {
                self.poll_at[index] = now + *interval;
                self.poll_column(index);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        if self.filter.is_some() {
            self.handle_filter_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('1') => self.panel = Panel::Contexts,
            KeyCode::Char('2') => self.panel = Panel::Repos,
            KeyCode::Char('3') => self.panel = Panel::Archived,
            KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => {
                self.panel = self.panel.cycle(-1)
            }
            KeyCode::Tab => self.panel = self.panel.cycle(1),
            KeyCode::Char('l') | KeyCode::Right => self.panel = self.panel.cycle(1),
            KeyCode::Char('j') | KeyCode::Down => {
                let table = self.table_mut(self.panel);
                table.move_cursor(table.cursor as isize + 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let table = self.table_mut(self.panel);
                table.move_cursor(table.cursor as isize - 1);
            }
            KeyCode::Char('g') => self.table_mut(self.panel).move_cursor(0),
            KeyCode::Char('G') => {
                let table = self.table_mut(self.panel);
                table.move_cursor(table.row_count() as isize - 1);
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('?') => {
                self.modal = Some(Modal::Help { panel: self.panel });
            }
            KeyCode::Char('/') => self.action_filter(),
            KeyCode::Char('n') if self.allow_mutation() => self.action_new(),
            KeyCode::Char('N') if self.allow_mutation() => self.action_new_from_base(),
            KeyCode::Enter => self.action_select(),
            _ => self.handle_panel_key(key),
        }
    }

    /// Enter on a panel row: the panel's primary action, mutation-gated.
    fn action_select(&mut self) {
        if !self.allow_mutation() {
            return;
        }
        match self.panel {
            Panel::Contexts => self.action_open(),
            Panel::Repos => self.action_new(),
            Panel::Archived => self.open_archived(),
        }
    }

    fn handle_panel_key(&mut self, key: KeyEvent) {
        let gated = self.allow_mutation();
        match (self.panel, key.code) {
            (Panel::Contexts, KeyCode::Char(' ')) if gated => self.action_open(),
            (Panel::Contexts, KeyCode::Char('o')) => self.action_open_pr(),
            (Panel::Contexts, KeyCode::Char('d')) if gated => self.action_archive(),
            (Panel::Contexts, KeyCode::Char('D')) if gated => self.action_delete(),
            (Panel::Repos, KeyCode::Char('a')) if gated => self.action_add_repo(),
            (Panel::Repos, KeyCode::Char('s')) if gated => self.action_set_default_repo(),
            (Panel::Repos, KeyCode::Char('d')) if gated => self.action_delete(),
            (Panel::Archived, KeyCode::Char('u')) if gated => self.action_unarchive(),
            (Panel::Archived, KeyCode::Char('d') | KeyCode::Char('D')) if gated => {
                self.action_delete()
            }
            (Panel::Archived, KeyCode::Char('e')) if gated => self.action_empty_archive(),
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match modal {
            Modal::Alert { message } => match key.code {
                KeyCode::Esc | KeyCode::Enter => {}
                _ => self.modal = Some(Modal::Alert { message }),
            },
            Modal::Help { panel } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => {}
                _ => self.modal = Some(Modal::Help { panel }),
            },
            Modal::Prompt {
                title,
                placeholder,
                mut input,
                mut replace_on_type,
                kind,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let value = input.value().trim().to_string();
                    if value.is_empty() {
                        self.modal = Some(Modal::Prompt {
                            title,
                            placeholder,
                            input,
                            replace_on_type,
                            kind,
                        });
                    } else {
                        self.submit_prompt(kind, value);
                    }
                }
                code => {
                    if replace_on_type {
                        if matches!(code, KeyCode::Char(_)) {
                            input = Input::default();
                        }
                        replace_on_type = false;
                    }
                    input.handle_event(&ratatui::crossterm::event::Event::Key(key));
                    self.modal = Some(Modal::Prompt {
                        title,
                        placeholder,
                        input,
                        replace_on_type,
                        kind,
                    });
                }
            },
            Modal::Confirm {
                message,
                confirm_label,
                mut selected,
                kind,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if selected == 0 {
                        self.confirm(kind);
                    }
                }
                KeyCode::Char('j' | 'l') | KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                    selected = (selected + 1) % 2;
                    self.modal = Some(Modal::Confirm {
                        message,
                        confirm_label,
                        selected,
                        kind,
                    });
                }
                KeyCode::Char('k' | 'h') | KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                    selected = (selected + 1) % 2;
                    self.modal = Some(Modal::Confirm {
                        message,
                        confirm_label,
                        selected,
                        kind,
                    });
                }
                _ => {
                    self.modal = Some(Modal::Confirm {
                        message,
                        confirm_label,
                        selected,
                        kind,
                    });
                }
            },
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.dismiss_filter(),
            KeyCode::Down => self.filter_cursor(1),
            KeyCode::Up => self.filter_cursor(-1),
            KeyCode::Enter => self.submit_filter(),
            _ => {
                if let Some(filter) = &mut self.filter {
                    filter
                        .query
                        .handle_event(&ratatui::crossterm::event::Event::Key(key));
                    self.apply_filter();
                }
            }
        }
    }

    fn handle_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.modal.is_some() {
                    self.click_modal(mouse.column, mouse.row);
                    return;
                }
                for panel in Panel::ALL {
                    if let Some(area) = self.areas.get(&panel)
                        && area.contains((mouse.column, mouse.row).into())
                    {
                        self.panel = panel;
                        // Rows start below the border and header.
                        let first_row = area.y + 2;
                        if mouse.row >= first_row {
                            let index =
                                (mouse.row - first_row) as usize + self.table(panel).view.offset();
                            self.table_mut(panel).move_cursor(index as isize);
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.modal.is_none() && self.filter.is_none() {
                    let table = self.table_mut(self.panel);
                    table.move_cursor(table.cursor as isize + 1);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.modal.is_none() && self.filter.is_none() {
                    let table = self.table_mut(self.panel);
                    table.move_cursor(table.cursor as isize - 1);
                }
            }
            _ => {}
        }
    }

    fn click_modal(&mut self, column: u16, row: u16) {
        let hit = self
            .button_areas
            .iter()
            .position(|area| area.contains((column, row).into()));
        if let Some(Modal::Confirm { .. }) = &self.modal
            && let Some(index) = hit
        {
            let Some(Modal::Confirm { kind, .. }) = self.modal.take() else {
                return;
            };
            if index == 0 {
                self.confirm(kind);
            }
        }
    }

    // ------------------------------------------------------------------
    // Actions

    fn action_open(&mut self) {
        let Some(ctx) = self.selected_context() else {
            return;
        };
        if !self.mux.can_open_in_place() {
            self.outcome = Some(Request::Open { name: ctx.name });
            self.quit = true;
            return;
        }
        if let Err(err) = self.mux.open(&ctx, None) {
            self.alert(err.to_string());
            return;
        }
        if self.exit_on_open {
            self.quit = true;
        }
    }

    /// The target repo: the hovered repo on the repos panel, else the default.
    ///
    /// With no default set, fall back to the hovered row's repo.
    fn repo_for_new(&mut self) -> Option<String> {
        if self.panel == Panel::Repos {
            return self.repos.selected_key().map(str::to_string);
        }
        if let Some(default) = repos::default_repo(&self.cfg) {
            return Some(default);
        }
        let hovered = match self.panel {
            Panel::Contexts => self.selected_context(),
            _ => self.selected_archived(),
        };
        if let Some(ctx) = hovered {
            return Some(ctx.repo);
        }
        self.repos.selected_key().map(str::to_string)
    }

    fn name_prompt(&mut self, repo: String) {
        let value = contexts::random_name(&self.cfg).unwrap_or_default();
        self.modal = Some(Modal::Prompt {
            title: format!("New context for {repo}"),
            placeholder: "name",
            input: Input::new(value),
            replace_on_type: true,
            kind: PromptKind::NewName { repo },
        });
    }

    fn action_new(&mut self) {
        match self.repo_for_new() {
            None => self.alert("no repos registered; press a to add one"),
            Some(repo) => self.name_prompt(repo),
        }
    }

    fn action_new_from_base(&mut self) {
        // The base prompt follows once the name is submitted.
        match self.repo_for_new() {
            None => self.alert("no repos registered; press a to add one"),
            Some(repo) => {
                self.name_prompt(repo);
                if let Some(Modal::Prompt { kind, .. }) = &mut self.modal {
                    let PromptKind::NewName { repo } = std::mem::replace(kind, PromptKind::AddRepo)
                    else {
                        return;
                    };
                    *kind = PromptKind::NewBase {
                        repo,
                        name: String::new(),
                    };
                }
            }
        }
    }

    fn submit_prompt(&mut self, kind: PromptKind, value: String) {
        match kind {
            PromptKind::NewName { repo } => self.create(repo, value, None),
            PromptKind::NewBase { repo, name } if name.is_empty() => {
                // The first prompt of the from-base flow gathered the name;
                // now ask for the base branch.
                self.modal = Some(Modal::Prompt {
                    title: format!("Base branch for {value}"),
                    placeholder: "branch",
                    input: Input::default(),
                    replace_on_type: false,
                    kind: PromptKind::NewBase { repo, name: value },
                });
            }
            PromptKind::NewBase { repo, name } => self.create(repo, name, Some(value)),
            PromptKind::AddRepo => {
                self.start_busy(Panel::Repos);
                let cfg = self.cfg.clone();
                let tx = self.tx.clone();
                self.workers += 1;
                spawn_worker(move || {
                    let alert = repos::add_repo(&cfg, &value, None)
                        .err()
                        .map(|err| err.to_string());
                    let _ = tx.send(Event::Worker(WorkerDone {
                        reload: true,
                        finish_busy: true,
                        alert,
                        finished: true,
                        ..WorkerDone::default()
                    }));
                });
            }
        }
    }

    /// Create in the background if we can stay running, else exit to the CLI.
    fn create(&mut self, repo: String, name: String, base: Option<String>) {
        if !self.mux.can_open_in_place() {
            self.outcome = Some(Request::New { repo, name, base });
            self.quit = true;
            return;
        }
        self.start_busy(Panel::Contexts);
        let cfg = self.cfg.clone();
        let mux = self.mux.clone();
        let tx = self.tx.clone();
        let exit_on_open = self.exit_on_open;
        self.workers += 1;
        spawn_worker(move || {
            let ctx = match contexts::create_context(&cfg, &repo, &name, base.as_deref()) {
                Err(err) => {
                    let _ = tx.send(Event::Worker(WorkerDone {
                        finish_busy: true,
                        alert: Some(err.to_string()),
                        finished: true,
                        ..WorkerDone::default()
                    }));
                    return;
                }
                Ok(ctx) => ctx,
            };
            let _ = tx.send(Event::Worker(WorkerDone {
                reload: true,
                finish_busy: true,
                ..WorkerDone::default()
            }));
            let opened = mux.open(&ctx, Some(&HashMap::new()));
            let _ = tx.send(Event::Worker(WorkerDone {
                alert: opened.as_ref().err().map(|err| err.to_string()),
                exit: opened.is_ok() && exit_on_open,
                finished: true,
                ..WorkerDone::default()
            }));
        });
    }

    /// Toggle the selected repo as the default for new contexts.
    fn action_set_default_repo(&mut self) {
        let Some(name) = self.repos.selected_key().map(str::to_string) else {
            return;
        };
        let current = repos::default_repo(&self.cfg);
        let target = if current.as_deref() == Some(name.as_str()) {
            None
        } else {
            Some(name.as_str())
        };
        if let Err(err) = repos::set_default_repo(&self.cfg, target) {
            self.alert(err.to_string());
            return;
        }
        self.reload();
    }

    fn action_add_repo(&mut self) {
        self.modal = Some(Modal::Prompt {
            title: "Add repo".to_string(),
            placeholder: "clone URL",
            input: Input::default(),
            replace_on_type: false,
            kind: PromptKind::AddRepo,
        });
    }

    fn action_unarchive(&mut self) {
        let Some(ctx) = self.selected_archived() else {
            return;
        };
        self.start_busy(Panel::Archived);
        self.unarchive_worker(ctx, false);
    }

    /// Enter on an archived context: unarchive it and open its session.
    fn open_archived(&mut self) {
        let Some(ctx) = self.selected_archived() else {
            return;
        };
        if !self.mux.can_open_in_place() {
            if let Err(err) = contexts::unarchive_context(&self.cfg, &ctx) {
                self.alert(err.to_string());
                return;
            }
            self.outcome = Some(Request::Open { name: ctx.name });
            self.quit = true;
            return;
        }
        self.start_busy(Panel::Archived);
        self.unarchive_worker(ctx, true);
    }

    fn unarchive_worker(&mut self, ctx: Context, open_after: bool) {
        let cfg = self.cfg.clone();
        let mux = self.mux.clone();
        let tx = self.tx.clone();
        let exit_on_open = self.exit_on_open;
        self.workers += 1;
        spawn_worker(move || {
            let restored = match contexts::unarchive_context(&cfg, &ctx) {
                Err(err) => {
                    let _ = tx.send(Event::Worker(WorkerDone {
                        finish_busy: true,
                        alert: Some(err.to_string()),
                        finished: true,
                        ..WorkerDone::default()
                    }));
                    return;
                }
                Ok(restored) => restored,
            };
            let _ = tx.send(Event::Worker(WorkerDone {
                reload: true,
                finish_busy: true,
                ..WorkerDone::default()
            }));
            if !open_after {
                let _ = tx.send(Event::Worker(WorkerDone {
                    finished: true,
                    ..WorkerDone::default()
                }));
                return;
            }
            let opened = mux.open(&restored, None);
            let _ = tx.send(Event::Worker(WorkerDone {
                alert: opened.as_ref().err().map(|err| err.to_string()),
                exit: opened.is_ok() && exit_on_open,
                finished: true,
                ..WorkerDone::default()
            }));
        });
    }

    fn action_empty_archive(&mut self) {
        let archived = contexts::list_archived(&self.cfg);
        if archived.is_empty() {
            return;
        }
        self.modal = Some(Modal::Confirm {
            message: format!(
                "Permanently delete all {} archived context(s)?",
                archived.len()
            ),
            confirm_label: "Empty",
            selected: 0,
            kind: ConfirmKind::EmptyArchive,
        });
    }

    fn action_delete(&mut self) {
        match self.panel {
            Panel::Repos => self.delete_repo_prompt(),
            Panel::Archived => {
                if let Some(ctx) = self.selected_archived() {
                    self.confirm_delete(ctx, false);
                }
            }
            Panel::Contexts => {
                if let Some(ctx) = self.selected_context() {
                    self.confirm_delete(ctx, true);
                }
            }
        }
    }

    fn confirm_delete(&mut self, ctx: Context, live: bool) {
        let mut problems = Vec::new();
        if contexts::is_dirty(&ctx).unwrap_or(false) {
            problems.push("uncommitted changes");
        }
        if !contexts::unpushed_commits(&ctx)
            .unwrap_or_default()
            .is_empty()
        {
            problems.push("unpushed commits");
        }
        let (message, label) = if problems.is_empty() {
            (format!("Permanently delete {}?", ctx.qualified()), "Delete")
        } else {
            (
                format!(
                    "{} has {}. Permanently delete anyway?",
                    ctx.qualified(),
                    problems.join(" and ")
                ),
                "Force delete",
            )
        };
        self.modal = Some(Modal::Confirm {
            message,
            confirm_label: label,
            selected: 0,
            kind: if live {
                ConfirmKind::DeleteLive(ctx)
            } else {
                ConfirmKind::DeleteArchived(ctx)
            },
        });
    }

    fn delete_repo_prompt(&mut self) {
        let Some(name) = self.repos.selected_key().map(str::to_string) else {
            return;
        };
        self.modal = Some(Modal::Confirm {
            message: format!("Remove repo '{name}'? Its contexts are left alone."),
            confirm_label: "Remove",
            selected: 0,
            kind: ConfirmKind::RemoveRepo(name),
        });
    }

    fn confirm(&mut self, kind: ConfirmKind) {
        match kind {
            ConfirmKind::DeleteLive(ctx) => {
                self.start_busy(Panel::Contexts);
                self.teardown_worker(ctx, Teardown::Delete);
            }
            ConfirmKind::DeleteArchived(ctx) => {
                self.start_busy(Panel::Archived);
                let tx = self.tx.clone();
                self.workers += 1;
                spawn_worker(move || {
                    let alert = contexts::remove_context(&ctx)
                        .err()
                        .map(|err| err.to_string());
                    let _ = tx.send(Event::Worker(WorkerDone {
                        reload: true,
                        finish_busy: true,
                        alert,
                        finished: true,
                        ..WorkerDone::default()
                    }));
                });
            }
            ConfirmKind::RemoveRepo(name) => {
                self.start_busy(Panel::Repos);
                let cfg = self.cfg.clone();
                let tx = self.tx.clone();
                self.workers += 1;
                spawn_worker(move || {
                    let alert = repos::remove_repo(&cfg, &name)
                        .err()
                        .map(|err| err.to_string());
                    let _ = tx.send(Event::Worker(WorkerDone {
                        reload: true,
                        finish_busy: true,
                        alert,
                        finished: true,
                        ..WorkerDone::default()
                    }));
                });
            }
            ConfirmKind::EmptyArchive => {
                self.start_busy(Panel::Archived);
                let cfg = self.cfg.clone();
                let tx = self.tx.clone();
                self.workers += 1;
                spawn_worker(move || {
                    let alert = contexts::empty_archive(&cfg)
                        .err()
                        .map(|err| err.to_string());
                    let _ = tx.send(Event::Worker(WorkerDone {
                        reload: true,
                        finish_busy: true,
                        alert,
                        finished: true,
                        ..WorkerDone::default()
                    }));
                });
            }
        }
    }

    /// Archive the selected context straight away; it is cheap to undo.
    fn action_archive(&mut self) {
        let Some(ctx) = self.selected_context() else {
            return;
        };
        self.start_busy(Panel::Contexts);
        self.teardown_worker(ctx, Teardown::Archive);
    }

    fn teardown_worker(&mut self, ctx: Context, mode: Teardown) {
        let cfg = self.cfg.clone();
        let mux = self.mux.clone();
        let tx = self.tx.clone();
        self.workers += 1;
        spawn_worker(move || {
            let alert = teardown(&cfg, mux.as_ref(), &ctx, mode)
                .err()
                .map(|err| err.to_string());
            let _ = tx.send(Event::Worker(WorkerDone {
                reload: true,
                finish_busy: true,
                alert,
                finished: true,
                ..WorkerDone::default()
            }));
        });
    }

    fn action_open_pr(&mut self) {
        if self.panel != Panel::Contexts {
            return;
        }
        let Some(ctx) = self.selected_context() else {
            return;
        };
        let tx = self.tx.clone();
        self.workers += 1;
        spawn_worker(move || {
            let alert = open_pr(&ctx).err();
            let _ = tx.send(Event::Worker(WorkerDone {
                alert,
                finished: true,
                ..WorkerDone::default()
            }));
        });
    }

    // ------------------------------------------------------------------
    // Filter

    fn action_filter(&mut self) {
        if self.filter.is_some() {
            return;
        }
        let target = self.panel;
        let rows = self.table(target).rows.clone();
        self.filter = Some(Filter {
            target,
            query: Input::default(),
            rows,
        });
    }

    fn drop_filter(&mut self) -> Option<Filter> {
        self.filter.take()
    }

    /// Restore the full panel, keeping the cursor on the filtered selection.
    fn dismiss_filter(&mut self) {
        let Some(filter) = self.drop_filter() else {
            return;
        };
        let selected = self.table(filter.target).selected_key().map(str::to_string);
        let table = self.table_mut(filter.target);
        table.rows = filter.rows;
        if let Some(key) = selected {
            table.cursor_to_key(&key);
        }
        self.panel = filter.target;
    }

    fn filter_cursor(&mut self, step: isize) {
        if let Some(filter) = &self.filter {
            let target = filter.target;
            let table = self.table_mut(target);
            table.move_cursor(table.cursor as isize + step);
        }
    }

    fn apply_filter(&mut self) {
        let Some(filter) = &self.filter else {
            return;
        };
        let target = filter.target;
        let with_repo = target != Panel::Repos;
        let query = filter.query.value().to_string();
        let matching: Vec<TableRow> = filter
            .rows
            .iter()
            .filter(|row| {
                let haystack = if with_repo {
                    format!(
                        "{}/{}",
                        row.cells.get(1).map(|c| c.text.as_str()).unwrap_or(""),
                        row.key
                    )
                } else {
                    row.key.clone()
                };
                fuzzy_match(&query, &haystack)
            })
            .cloned()
            .collect();
        let table = self.table_mut(target);
        table.rows = matching;
        table.move_cursor(table.cursor as isize);
    }

    fn submit_filter(&mut self) {
        let Some(filter) = &self.filter else {
            return;
        };
        let target = filter.target;
        if self.table(target).selected_key().is_none() {
            return;
        }
        self.dismiss_filter();
        self.panel = target;
        self.action_select();
    }

    // ------------------------------------------------------------------
    // Run loop

    pub fn run(mut self) -> std::io::Result<Option<Request>> {
        use ratatui::crossterm::event::{
            DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyEventKind,
        };
        use ratatui::crossterm::execute;

        let mut terminal = ratatui::init();
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
        self.mount();

        // Input thread: raw crossterm events onto the app channel.
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = ratatui::crossterm::event::read() {
                let forwarded = match event {
                    CtEvent::Key(key) if key.kind != KeyEventKind::Release => Event::Key(key),
                    CtEvent::Mouse(mouse) => Event::Mouse(mouse),
                    CtEvent::Resize(_, _) => Event::Redraw,
                    _ => continue,
                };
                if tx.send(forwarded).is_err() {
                    break;
                }
            }
        });
        // Tick thread: the spinner beat and the status poll clock.
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
        });

        let result = loop {
            terminal.draw(|frame| {
                let area = frame.area();
                let buffer = frame.buffer_mut();
                self.render(area, buffer);
            })?;
            let Ok(event) = self.rx.recv() else {
                break None;
            };
            self.handle(event);
            while let Ok(event) = self.rx.try_recv() {
                self.handle(event);
            }
            if self.quit {
                break self.outcome.take();
            }
        };

        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        // Quitting must cancel an in-flight create, not wait out its fetch.
        crate::git::kill_inflight();
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Rendering

    pub fn render(&mut self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let filter_height = u16::from(self.filter.is_some());
        let [contexts_area, bottom_area, filter_area, footer_area] = Layout::vertical([
            Constraint::Fill(7),
            Constraint::Fill(3),
            Constraint::Length(filter_height),
            Constraint::Length(1),
        ])
        .areas(area);
        let [repos_area, archived_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(bottom_area);
        self.areas = HashMap::from([
            (Panel::Contexts, contexts_area),
            (Panel::Repos, repos_area),
            (Panel::Archived, archived_area),
        ]);
        self.render_panel(Panel::Contexts, contexts_area, buffer);
        self.render_panel(Panel::Repos, repos_area, buffer);
        self.render_panel(Panel::Archived, archived_area, buffer);
        if self.filter.is_some() {
            self.render_filter(filter_area, buffer);
        }
        self.render_footer(footer_area, buffer);
        self.render_modal(area, buffer);
    }

    fn render_panel(&mut self, panel: Panel, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let theme = self.cfg.theme.clone();
        let busy = self.busy.contains(&panel);
        let focused = self.panel == panel && self.filter.is_none() && self.modal.is_none();
        let border = if focused {
            theme_color(&theme.border_active)
        } else {
            theme_color(&theme.border_inactive)
        };
        let mut title = panel.title().to_string();
        if busy {
            let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
            title = format!("{title} {frame}");
        }
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .title(title);
        let inner = block.inner(area);
        block.render_widget(area, buffer);

        let table = self.table_mut(panel);
        let columns = table.headers.len();
        let mut widths = vec![0usize; columns];
        for (index, header) in table.headers.iter().enumerate() {
            widths[index] = header.chars().count();
        }
        for row in &table.rows {
            for (index, cell) in row.cells.iter().enumerate() {
                widths[index] = widths[index].max(cell.text.chars().count());
            }
        }
        let foreground = theme_color(&theme.foreground);
        let header = Row::new(
            table
                .headers
                .iter()
                .map(|header| TableCell::from(header.as_str())),
        )
        .style(Style::default().fg(foreground).bold());
        let mut base = Style::default().fg(foreground);
        if busy {
            base = base.add_modifier(Modifier::DIM);
        }
        let rows: Vec<Row> = table
            .rows
            .iter()
            .map(|row| {
                Row::new(row.cells.iter().map(|cell| {
                    let mut style = base;
                    if let Some(name) = cell.style {
                        style = style.patch(status_style(name));
                    }
                    TableCell::from(cell.text.clone()).style(style)
                }))
            })
            .collect();
        // lazygit-style selection: only the focused panel shows its cursor;
        // bright-bold status colours keep their contrast on it.
        let highlight = if focused {
            Style::default()
                .bg(theme_color(&theme.selection))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        table
            .view
            .select((!table.rows.is_empty()).then_some(table.cursor));
        let widget = Table::new(
            rows,
            widths
                .iter()
                .map(|width| Constraint::Length(*width as u16))
                .collect::<Vec<_>>(),
        )
        .header(header)
        .column_spacing(2)
        .row_highlight_style(highlight);
        ratatui::widgets::StatefulWidget::render(widget, inner, buffer, &mut table.view);
    }

    fn render_filter(&self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let Some(filter) = &self.filter else {
            return;
        };
        let value = filter.query.value();
        let line = if value.is_empty() {
            Line::from(Span::styled(" filter", Style::default().dim()))
        } else {
            Line::from(format!(" {value}"))
        };
        Paragraph::new(line).render_widget(area, buffer);
    }

    fn render_footer(&self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let bindings: &[(&str, &str)] = match self.panel {
            Panel::Contexts => &[
                ("space", "Open"),
                ("o", "Open PR"),
                ("d", "Archive"),
                ("D", "Delete"),
                ("n", "New context"),
                ("/", "Filter"),
                ("r", "Refresh"),
                ("q", "Quit"),
                ("?", "Help"),
            ],
            Panel::Repos => &[
                ("a", "Add repo"),
                ("s", "Set default"),
                ("d", "Remove repo"),
                ("n", "New context"),
                ("/", "Filter"),
                ("r", "Refresh"),
                ("q", "Quit"),
                ("?", "Help"),
            ],
            Panel::Archived => &[
                ("u", "Unarchive"),
                ("d", "Delete"),
                ("e", "Empty"),
                ("n", "New context"),
                ("/", "Filter"),
                ("r", "Refresh"),
                ("q", "Quit"),
                ("?", "Help"),
            ],
        };
        let mut spans = Vec::new();
        for (key, label) in bindings {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(*key, Style::default().bold()));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(*label, Style::default().dim()));
        }
        Paragraph::new(Line::from(spans)).render_widget(area, buffer);
    }

    fn render_modal(&mut self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        self.button_areas.clear();
        let Some(modal) = &self.modal else {
            return;
        };
        let width = 60.min(area.width.saturating_sub(4)).max(20);
        let inner_width = width.saturating_sub(6) as usize;
        // Title, body, whether an input field follows, confirm buttons.
        type ModalParts<'a> = (
            Option<String>,
            Vec<Line<'a>>,
            bool,
            Option<(&'a str, usize)>,
        );
        let (title, body_lines, has_input, buttons): ModalParts = match modal {
            Modal::Prompt { title, .. } => (Some(title.clone()), Vec::new(), true, None),
            Modal::Alert { message } => {
                // Errors quote whatever failed (git argv, paths); the text
                // renders verbatim, never as markup.
                (None, wrapped_lines(message, inner_width), false, None)
            }
            Modal::Help { panel } => {
                let mut lines = vec![Line::from(format!("Keybindings ({})", panel.name()))];
                lines.push(Line::default());
                for (key, desc) in panel_keybindings(*panel) {
                    lines.push(Line::from(format!("{key:<16}{desc}")));
                }
                (None, lines, false, None)
            }
            Modal::Confirm {
                message,
                confirm_label,
                selected,
                ..
            } => (
                None,
                wrapped_lines(message, inner_width),
                false,
                Some((confirm_label, *selected)),
            ),
        };
        let mut height = body_lines.len() as u16 + 4;
        if title.is_some() {
            height += 2;
        }
        if has_input {
            height += 3;
        }
        if buttons.is_some() {
            height += 2;
        }
        let height = height.min(area.height);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };
        Clear.render_widget(popup, buffer);
        let block = Block::bordered().border_type(BorderType::Rounded);
        let inner = block.inner(popup);
        block.render_widget(popup, buffer);
        let inner = inner.inner(Margin::new(2, 1));
        let mut y = inner.y;
        if let Some(title) = title {
            Paragraph::new(title).render_widget(
                Rect {
                    height: 1,
                    y,
                    ..inner
                },
                buffer,
            );
            y += 2;
        }
        if !body_lines.is_empty() {
            let height = body_lines.len() as u16;
            Paragraph::new(body_lines)
                .wrap(Wrap { trim: false })
                .render_widget(Rect { height, y, ..inner }, buffer);
            y += height + 1;
        }
        if has_input
            && let Some(Modal::Prompt {
                input, placeholder, ..
            }) = &self.modal
        {
            let field = Rect {
                height: 3,
                y,
                ..inner
            };
            let border_active = theme_color(&self.cfg.theme.border_active);
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_active));
            let text_area = block.inner(field);
            block.render_widget(field, buffer);
            let value = input.value();
            let line = if value.is_empty() {
                Line::from(Span::styled(*placeholder, Style::default().dim()))
            } else {
                Line::from(value.to_string())
            };
            Paragraph::new(line).render_widget(text_area, buffer);
            // A visible cursor: invert the cell the input writes next.
            let cursor_x = text_area.x + (input.visual_cursor() as u16).min(text_area.width - 1);
            if let Some(cell) = buffer.cell_mut((cursor_x, text_area.y)) {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
        if let Some((label, selected)) = buttons {
            let confirm = format!("[ {label} ]");
            let cancel = "[ Cancel ]".to_string();
            let total = (confirm.chars().count() + 2 + cancel.chars().count()) as u16;
            let start = inner.x + inner.width.saturating_sub(total);
            let confirm_area = Rect {
                x: start,
                y,
                width: confirm.chars().count() as u16,
                height: 1,
            };
            let cancel_area = Rect {
                x: start + confirm_area.width + 2,
                y,
                width: cancel.chars().count() as u16,
                height: 1,
            };
            let selected_style = Style::default().add_modifier(Modifier::REVERSED);
            Paragraph::new(Span::styled(
                confirm,
                if selected == 0 {
                    selected_style.fg(Color::LightRed)
                } else {
                    Style::default().fg(Color::LightRed)
                },
            ))
            .render_widget(confirm_area, buffer);
            Paragraph::new(Span::styled(
                cancel,
                if selected == 1 {
                    selected_style
                } else {
                    Style::default()
                },
            ))
            .render_widget(cancel_area, buffer);
            self.button_areas = vec![confirm_area, cancel_area];
        }
    }
}

enum Teardown {
    Archive,
    Delete,
}

fn teardown(cfg: &Config, mux: &dyn Multiplexer, ctx: &Context, mode: Teardown) -> CtxResult<()> {
    if mux.exists(ctx) && mux.is_current(ctx) {
        // Killing our own session takes the TUI (and the client) down with
        // it, so land the client elsewhere first.
        switch_away(cfg, mux, ctx);
    }
    // Kill last: killing our own session ends the TUI, so nothing after the
    // kill is guaranteed to run. Kill even when the removal fails half-way;
    // the startup sweep finishes the removal.
    let removed = match mode {
        Teardown::Archive => contexts::archive_context(cfg, ctx).map(|_| ()),
        Teardown::Delete => contexts::remove_context(ctx),
    };
    if mux.exists(ctx) {
        mux.kill(ctx)?;
    }
    removed
}

/// Re-point the client at the most recent other running session.
fn switch_away(cfg: &Config, mux: &dyn Multiplexer, ctx: &Context) {
    for other in contexts::list_contexts(cfg) {
        if other.name == ctx.name || !mux.exists(&other) {
            continue;
        }
        if mux.open(&other, None).is_ok() {
            return;
        }
    }
}

fn open_pr(ctx: &Context) -> Result<(), String> {
    let remote = new_command("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&ctx.path)
        .output()
        .map_err(|err| err.to_string())?;
    let command = forge::pr_view_command(String::from_utf8_lossy(&remote.stdout).trim());
    let result = new_command(&command[0])
        .args(&command[1..])
        .current_dir(&ctx.path)
        .output()
        .map_err(|err| err.to_string())?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            "could not open the PR".to_string()
        } else {
            detail.to_string()
        });
    }
    Ok(())
}

fn fetch_cell(cfg: &Config, ctx: &Context, index: usize) -> CellValue {
    if index == 0 {
        // Colour a status cell if its value is a well-known status word.
        let state = status::git_state(ctx);
        let style = status::STATUS_STYLES
            .iter()
            .find(|(word, _)| *word == state)
            .map(|(_, style)| *style);
        return CellValue::styled(state, style);
    }
    let column = &cfg.status[index - 1];
    match status::column_status(ctx, column) {
        Some(cell) if !cell.is_empty() => {
            let display = status::cell_icon(column, &cell, cfg.nerd_font);
            CellValue::styled(display, status::cell_style(&cell))
        }
        _ => CellValue::default(),
    }
}

/// True when the query's characters appear in order within the name.
fn fuzzy_match(query: &str, name: &str) -> bool {
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let mut position = 0;
    for ch in query.to_lowercase().chars() {
        match name[position..].iter().position(|c| *c == ch) {
            Some(offset) => position += offset + 1,
            None => return false,
        }
    }
    true
}

fn wrapped_lines(message: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(10);
    let mut lines = Vec::new();
    for raw in message.lines() {
        let chars: Vec<char> = raw.chars().collect();
        if chars.is_empty() {
            lines.push(Line::default());
            continue;
        }
        for chunk in chars.chunks(width) {
            lines.push(Line::from(chunk.iter().collect::<String>()));
        }
    }
    lines
}

fn panel_keybindings(panel: Panel) -> Vec<(&'static str, &'static str)> {
    let panel_bindings: &[(&str, &str)] = match panel {
        Panel::Contexts => &[
            ("enter / space", "open context"),
            ("o", "open the PR in the browser"),
            ("n", "new context"),
            ("N", "new context from a base branch"),
            ("d", "archive context"),
            ("D", "permanently delete context"),
        ],
        Panel::Repos => &[
            ("enter / n", "new context"),
            ("N", "new context from a base branch"),
            ("a", "add repo"),
            ("s", "set / clear default repo"),
            ("d", "remove repo"),
        ],
        Panel::Archived => &[
            ("enter", "unarchive and open context"),
            ("u", "unarchive context"),
            ("n", "new context"),
            ("N", "new context from a base branch"),
            ("d / D", "permanently delete context"),
            ("e", "empty the archive"),
        ],
    };
    let common: &[(&str, &str)] = &[
        ("j / k / ↓ / ↑", "move within panel"),
        ("g / G", "jump to top / bottom"),
        ("h / l / ← / →", "switch panel"),
        ("1 / 2 / 3", "jump to panel"),
        ("/", "fuzzy filter by repo and name"),
        ("r", "refresh"),
        ("?", "this help"),
        ("q / ctrl+c", "quit"),
    ];
    panel_bindings.iter().chain(common).copied().collect()
}

/// Map a theme colour (ansi name or hex) onto a terminal colour.
fn theme_color(name: &str) -> Color {
    if let Some(hex) = name.strip_prefix('#')
        && hex.len() == 6
        && let Ok(value) = u32::from_str_radix(hex, 16)
    {
        return Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8);
    }
    match name {
        "ansi_default" => Color::Reset,
        "ansi_black" => Color::Black,
        "ansi_red" => Color::Red,
        "ansi_green" => Color::Green,
        "ansi_yellow" => Color::Yellow,
        "ansi_blue" => Color::Blue,
        "ansi_magenta" => Color::Magenta,
        "ansi_cyan" => Color::Cyan,
        "ansi_white" => Color::White,
        _ => Color::Reset,
    }
}

/// A status vocabulary style ("bold bright_green") as a terminal style.
fn status_style(name: &str) -> Style {
    let mut style = Style::default();
    for word in name.split_whitespace() {
        style = match word {
            "bold" => style.add_modifier(Modifier::BOLD),
            "bright_green" => style.fg(Color::LightGreen),
            "bright_cyan" => style.fg(Color::LightCyan),
            "bright_yellow" => style.fg(Color::LightYellow),
            "bright_red" => style.fg(Color::LightRed),
            "bright_magenta" => style.fg(Color::LightMagenta),
            "bright_black" => style.fg(Color::DarkGray),
            _ => style,
        };
    }
    style
}

/// Widget rendering without a Frame, so tests can draw into a plain buffer.
trait RenderWidget {
    fn render_widget(self, area: Rect, buffer: &mut ratatui::buffer::Buffer);
}

impl<W: ratatui::widgets::Widget> RenderWidget for W {
    fn render_widget(self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        self.render(area, buffer);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ratatui::buffer::Buffer;

    use super::*;
    use crate::config::StatusColumn;
    use crate::multiplexer::MultiplexerError;
    use crate::testutil::{TestEnv, test_env};

    #[derive(Default)]
    struct MuxState {
        calls: Vec<(String, String)>,
        path_present_at_kill: Option<bool>,
    }

    /// Test double: canned exists()/is_current() answers, recorded calls.
    struct TestMux {
        exists: bool,
        current: Option<String>,
        state: Mutex<MuxState>,
    }

    impl TestMux {
        fn stub() -> Arc<TestMux> {
            Arc::new(TestMux {
                exists: false,
                current: None,
                state: Mutex::new(MuxState::default()),
            })
        }

        fn recording(current: Option<&str>) -> Arc<TestMux> {
            Arc::new(TestMux {
                exists: true,
                current: current.map(str::to_string),
                state: Mutex::new(MuxState::default()),
            })
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.state.lock().unwrap().calls.clone()
        }
    }

    impl Multiplexer for TestMux {
        fn can_open_in_place(&self) -> bool {
            true
        }

        fn exists(&self, _ctx: &Context) -> bool {
            self.exists
        }

        fn is_current(&self, ctx: &Context) -> bool {
            self.current.as_deref() == Some(ctx.name.as_str())
        }

        fn create(
            &self,
            _ctx: &Context,
            _values: Option<&HashMap<String, String>>,
        ) -> Result<(), MultiplexerError> {
            Ok(())
        }

        fn open(
            &self,
            ctx: &Context,
            _values: Option<&HashMap<String, String>>,
        ) -> Result<(), MultiplexerError> {
            self.state
                .lock()
                .unwrap()
                .calls
                .push(("open".to_string(), ctx.name.clone()));
            Ok(())
        }

        fn kill(&self, ctx: &Context) -> Result<(), MultiplexerError> {
            let mut state = self.state.lock().unwrap();
            state.path_present_at_kill = Some(ctx.path.exists());
            state.calls.push(("kill".to_string(), ctx.name.clone()));
            Ok(())
        }
    }

    fn registered() -> (TestEnv, std::path::PathBuf) {
        let env = test_env();
        let origin = env.origin();
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        (env, origin)
    }

    fn create(env: &TestEnv, repo: &str, name: &str) -> Context {
        contexts::create_context(&env.cfg, repo, name, None).unwrap()
    }

    fn slow_status_cfg(env: &TestEnv) -> Config {
        let mut cfg = env.cfg.clone();
        cfg.status = vec![StatusColumn {
            name: "slow".to_string(),
            command: Some("sleep 0.5; echo hi".to_string()),
            builtin: None,
            interval: None,
        }];
        cfg
    }

    impl CtxTui {
        fn key(&mut self, code: KeyCode) {
            self.handle(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        }

        fn keys(&mut self, codes: &[KeyCode]) {
            for code in codes {
                self.key(*code);
            }
        }

        fn idle(&self) -> bool {
            self.workers == 0 && self.fetching.is_empty()
        }

        /// Pump worker events until the predicate holds or the deadline passes.
        fn drain_until(&mut self, mut pred: impl FnMut(&CtxTui) -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if pred(self) {
                    return true;
                }
                if Instant::now() > deadline {
                    return false;
                }
                if let Ok(event) = self.rx.recv_timeout(Duration::from_millis(50)) {
                    self.handle(event);
                }
            }
        }

        fn drain_idle(&mut self) {
            assert!(self.drain_until(CtxTui::idle), "workers never finished");
        }

        fn slow_cells(&self) -> Vec<String> {
            self.contexts
                .rows
                .iter()
                .map(|row| row.cells[4].text.clone())
                .collect()
        }
    }

    fn app(cfg: &Config, mux: Arc<TestMux>) -> CtxTui {
        let mut app = CtxTui::new(cfg.clone(), mux, false);
        app.mount();
        app
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn render(app: &mut CtxTui) -> String {
        let area = Rect::new(0, 0, 100, 30);
        let mut buffer = Buffer::empty(area);
        app.render(area, &mut buffer);
        buffer_text(&buffer)
    }

    #[test]
    fn panels_are_populated_before_the_statuses_are() {
        // Rows must be there to act on straight away, slow providers or not.
        let (env, _origin) = registered();
        let cfg = slow_status_cfg(&env);
        for name in ["one", "two"] {
            contexts::create_context(&cfg, "origin", name, None).unwrap();
        }

        let mut app = app(&cfg, TestMux::stub());

        assert_eq!(app.contexts.row_count(), 2);
        assert_eq!(app.repos.row_count(), 1);
        assert_eq!(app.slow_cells(), ["", ""]);

        assert!(
            app.drain_until(|app| app.slow_cells() == ["hi", "hi"]),
            "statuses never filled in"
        );
    }

    #[test]
    fn arrow_keys_navigate_like_the_vim_keys() {
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Down);
        assert_eq!(app.contexts.cursor, 1);
        app.key(KeyCode::Up);
        assert_eq!(app.contexts.cursor, 0);

        app.key(KeyCode::Right);
        assert_eq!(app.panel, Panel::Repos);
        app.key(KeyCode::Right);
        assert_eq!(app.panel, Panel::Archived);
        app.key(KeyCode::Left);
        assert_eq!(app.panel, Panel::Repos);

        app.key(KeyCode::Char('d'));
        let selected = |app: &CtxTui| match &app.modal {
            Some(Modal::Confirm { selected, .. }) => *selected,
            _ => panic!("expected a confirm dialog"),
        };
        let first = selected(&app);
        app.key(KeyCode::Right);
        assert_eq!(selected(&app), first + 1);
        app.key(KeyCode::Up);
        assert_eq!(selected(&app), first);
    }

    #[test]
    fn tab_cycles_panels_like_textual_focus() {
        let (env, _origin) = registered();
        create(&env, "origin", "one");
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Tab);
        assert_eq!(app.panel, Panel::Repos);
        app.key(KeyCode::Tab);
        assert_eq!(app.panel, Panel::Archived);
        app.key(KeyCode::Tab);
        assert_eq!(app.panel, Panel::Contexts);
        app.key(KeyCode::BackTab);
        assert_eq!(app.panel, Panel::Archived);

        // Inside a confirm dialog, tab moves between the buttons instead.
        app.panel = Panel::Contexts;
        app.key(KeyCode::Char('D'));
        let selected = |app: &CtxTui| match &app.modal {
            Some(Modal::Confirm { selected, .. }) => *selected,
            _ => panic!("expected a confirm dialog"),
        };
        assert_eq!(selected(&app), 0);
        app.key(KeyCode::Tab);
        assert_eq!(selected(&app), 1);
        app.key(KeyCode::Tab);
        assert_eq!(selected(&app), 0, "button focus must wrap around");
    }

    #[test]
    fn alerts_show_bracketed_error_text_verbatim() {
        // Errors often quote a git command; its brackets must render as text.
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());
        let message = "Command '[git, -c, http.lowSpeedLimit=1000, fetch, origin]' failed";

        app.alert(message);
        let text = render(&mut app);

        assert!(text.contains("'[git, -c,"), "alert text missing: {text}");
    }

    #[test]
    fn archiving_another_context_does_not_switch() {
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }
        let ctx = contexts::find_context(&env.cfg, "one").unwrap();
        let mux = TestMux::recording(Some("two"));
        let mut app = app(&env.cfg, mux.clone());

        app.teardown_worker(ctx, Teardown::Archive);
        app.drain_idle();

        assert_eq!(mux.calls(), [("kill".to_string(), "one".to_string())]);
        assert!(contexts::find_context(&env.cfg, "one").is_err());
    }

    #[test]
    fn theme_colours_reach_the_terminal_styles() {
        assert_eq!(theme_color("#2d3f76"), Color::Rgb(0x2d, 0x3f, 0x76));
        assert_eq!(theme_color("ansi_default"), Color::Reset);
        assert_eq!(theme_color("ansi_blue"), Color::Blue);
    }

    #[test]
    fn current_context_is_pinned_and_cursor_starts_below_it() {
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }

        let app = app(&env.cfg, TestMux::recording(Some("one")));

        assert_eq!(
            app.contexts.rows[0].key, "one",
            "the attached context must be the top row"
        );
        assert_eq!(
            app.contexts.cursor, 1,
            "the cursor must start on the next context"
        );
    }

    #[test]
    fn cursor_starts_on_top_without_a_current_context() {
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }

        let app = app(&env.cfg, TestMux::stub());

        assert_eq!(app.contexts.cursor, 0);
    }

    #[test]
    fn new_prompt_prefills_a_generated_name() {
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('n'));
        let name = match &app.modal {
            Some(Modal::Prompt { input, .. }) => input.value().to_string(),
            _ => panic!("expected the name prompt"),
        };
        assert!(
            !name.is_empty(),
            "the prompt must pre-fill a generated name"
        );
        app.key(KeyCode::Enter);
        app.drain_idle();

        assert!(contexts::find_context(&env.cfg, &name).is_ok());
    }

    #[test]
    fn typing_replaces_the_prefilled_name() {
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('n'));
        app.key(KeyCode::Char('x'));

        match &app.modal {
            Some(Modal::Prompt { input, .. }) => assert_eq!(input.value(), "x"),
            _ => panic!("expected the name prompt"),
        }
    }

    #[test]
    fn new_context_uses_the_default_repo_off_the_repos_panel() {
        let (env, _origin) = registered();
        let other = env.make_origin("other", false);
        repos::add_repo(&env.cfg, &other.to_string_lossy(), None).unwrap();
        create(&env, "origin", "one");
        repos::set_default_repo(&env.cfg, Some("other")).unwrap();
        let mut app = app(&env.cfg, TestMux::stub());

        assert_eq!(
            app.repo_for_new().as_deref(),
            Some("other"),
            "contexts panel must use the default"
        );
        app.panel = Panel::Repos;
        app.key(KeyCode::Char('j'));
        assert_eq!(
            app.repo_for_new().as_deref(),
            Some("origin"),
            "repos panel must use the hovered repo"
        );
    }

    #[test]
    fn default_repo_sorts_first() {
        let (env, _origin) = registered();
        let other = env.make_origin("aaa", false);
        repos::add_repo(&env.cfg, &other.to_string_lossy(), None).unwrap();
        repos::set_default_repo(&env.cfg, Some("origin")).unwrap();

        let app = app(&env.cfg, TestMux::stub());

        assert_eq!(
            app.repos.selected_key(),
            Some("origin"),
            "default must be the top row"
        );
    }

    #[test]
    fn s_toggles_the_default_repo() {
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());

        app.panel = Panel::Repos;
        app.key(KeyCode::Char('s'));
        assert_eq!(repos::default_repo(&env.cfg).as_deref(), Some("origin"));
        app.key(KeyCode::Char('s'));
        assert_eq!(repos::default_repo(&env.cfg), None);
    }

    #[test]
    fn o_opens_the_pr_in_the_browser() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        let log = env.root().join("gh-args");
        let _gh = env.fake_cli("gh", &format!("echo \"$@\" > {}", log.display()));
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('o'));
        app.drain_idle();

        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "pr view --web"
        );
        assert_eq!(
            contexts::find_context(&env.cfg, "one").unwrap().path,
            ctx.path
        );
    }

    #[test]
    fn o_uses_the_forge_from_the_remote() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        crate::testutil::git(
            &[
                "remote",
                "set-url",
                "origin",
                "git@gitlab.com:jane/tool.git",
            ],
            &ctx.path,
        );
        let log = env.root().join("glab-args");
        let _glab = env.fake_cli("glab", &format!("echo \"$@\" > {}", log.display()));
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('o'));
        app.drain_idle();

        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "mr view --web"
        );
    }

    #[test]
    fn archive_key_archives_without_a_prompt() {
        let (env, _origin) = registered();
        create(&env, "origin", "one");
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('d'));
        app.drain_idle();

        assert!(contexts::find_archived(&env.cfg, "one").is_ok());
    }

    #[test]
    fn delete_key_asks_for_confirmation() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        contexts::archive_context(&env.cfg, &ctx).unwrap();
        let mut app = app(&env.cfg, TestMux::stub());

        app.panel = Panel::Archived;
        app.key(KeyCode::Char('d'));
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        app.key(KeyCode::Esc);
        app.drain_idle();

        assert!(contexts::find_archived(&env.cfg, "one").is_ok());
    }

    #[test]
    fn shift_delete_key_on_contexts_asks_for_confirmation() {
        let (env, _origin) = registered();
        create(&env, "origin", "one");
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('D'));
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        app.key(KeyCode::Esc);
        app.drain_idle();

        assert!(contexts::find_context(&env.cfg, "one").is_ok());
    }

    #[test]
    fn confirming_delete_removes_the_checkout() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('D'));
        app.key(KeyCode::Enter);
        app.drain_idle();

        assert!(!ctx.path.exists());
        assert!(contexts::find_context(&env.cfg, "one").is_err());
    }

    #[test]
    fn startup_sweeps_interrupted_deletions() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        let leftover = ctx.path.with_file_name("one.deleting");
        std::fs::rename(&ctx.path, &leftover).unwrap();

        let mut app = app(&env.cfg, TestMux::stub());
        app.drain_idle();

        assert!(!leftover.exists());
    }

    #[test]
    fn add_repo_key_is_local_to_the_repos_panel() {
        // `a` opens the add-repo prompt only while the repos panel is focused.
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('a'));
        assert!(app.modal.is_none(), "a must be inert off the repos panel");

        app.panel = Panel::Repos;
        app.key(KeyCode::Char('a'));
        assert!(matches!(app.modal, Some(Modal::Prompt { .. })));
    }

    #[test]
    fn archiving_the_current_context_switches_away_then_kills() {
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }
        let ctx = contexts::find_context(&env.cfg, "one").unwrap();
        let mux = TestMux::recording(Some("one"));
        let mut app = app(&env.cfg, mux.clone());

        app.teardown_worker(ctx, Teardown::Archive);
        app.drain_idle();

        assert_eq!(
            mux.calls(),
            [
                ("open".to_string(), "two".to_string()),
                ("kill".to_string(), "one".to_string()),
            ]
        );
        // Killing our own session ends the process, so the move must have
        // landed by the time the kill happens.
        assert_eq!(
            mux.state.lock().unwrap().path_present_at_kill,
            Some(false),
            "the move must come before the kill"
        );
        assert!(contexts::find_archived(&env.cfg, "one").is_ok());
    }

    #[test]
    fn archiving_kills_the_session_even_when_the_move_fails() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");
        // An occupied archive path fails the move before anything happens.
        std::fs::create_dir_all(env.cfg.archive_dir.join("origin").join("one")).unwrap();
        let mux = TestMux::recording(None);
        let mut app = app(&env.cfg, mux.clone());

        app.teardown_worker(ctx.clone(), Teardown::Archive);
        app.drain_idle();

        assert_eq!(mux.calls(), [("kill".to_string(), "one".to_string())]);
        assert!(ctx.path.exists());
    }

    #[test]
    fn archiving_the_current_context_leaves_no_stale_busy_state() {
        // A TUI in a tmux popup outlives its session's kill; it must repaint.
        let (env, _origin) = registered();
        for name in ["one", "two"] {
            create(&env, "origin", name);
        }
        let ctx = contexts::find_context(&env.cfg, "one").unwrap();
        let mut app = app(&env.cfg, TestMux::recording(Some("one")));

        app.start_busy(Panel::Contexts);
        app.teardown_worker(ctx, Teardown::Archive);
        app.drain_idle();

        assert!(
            app.busy.is_empty(),
            "the panel stayed dimmed after the archive"
        );
        assert_eq!(app.contexts.row_count(), 1);
    }

    #[test]
    fn slash_filters_and_enter_opens_the_match() {
        let (env, _origin) = registered();
        for name in ["alpha", "beta"] {
            create(&env, "origin", name);
        }
        let mux = TestMux::recording(None);
        let mut app = app(&env.cfg, mux.clone());

        app.keys(&[KeyCode::Char('/'), KeyCode::Char('b'), KeyCode::Char('t')]);
        assert_eq!(
            app.contexts.row_count(),
            1,
            "only the fuzzy match may remain"
        );
        app.key(KeyCode::Enter);

        assert!(
            mux.calls()
                .contains(&("open".to_string(), "beta".to_string()))
        );
        assert_eq!(
            app.contexts.row_count(),
            2,
            "the filter must clear after opening"
        );
    }

    #[test]
    fn escape_clears_the_filter() {
        let (env, _origin) = registered();
        for name in ["alpha", "beta"] {
            create(&env, "origin", name);
        }
        let mut app = app(&env.cfg, TestMux::stub());

        app.keys(&[KeyCode::Char('/'), KeyCode::Char('b')]);
        assert_eq!(app.contexts.row_count(), 1);
        app.key(KeyCode::Esc);
        assert_eq!(app.contexts.row_count(), 2);
        assert_eq!(app.panel, Panel::Contexts);
    }

    #[test]
    fn enter_with_no_matches_keeps_filtering() {
        let (env, _origin) = registered();
        create(&env, "origin", "alpha");
        let mux = TestMux::recording(None);
        let mut app = app(&env.cfg, mux.clone());

        app.keys(&[KeyCode::Char('/'), KeyCode::Char('z')]);
        assert_eq!(app.contexts.row_count(), 0);
        app.key(KeyCode::Enter);

        assert!(mux.calls().is_empty());
        assert_eq!(app.contexts.row_count(), 0, "the filter must stay active");
        assert!(app.filter.is_some());
    }

    #[test]
    fn filter_matches_the_repo_too() {
        let (env, _origin) = registered();
        let other = env.make_origin("other", false);
        repos::add_repo(&env.cfg, &other.to_string_lossy(), None).unwrap();
        create(&env, "origin", "alpha");
        create(&env, "other", "beta");
        let mut app = app(&env.cfg, TestMux::stub());

        app.keys(&[
            KeyCode::Char('/'),
            KeyCode::Char('o'),
            KeyCode::Char('t'),
            KeyCode::Char('h'),
        ]);

        assert_eq!(app.contexts.row_count(), 1);
        assert_eq!(app.contexts.selected_key(), Some("beta"));
    }

    #[test]
    fn filter_is_panel_scoped() {
        let (env, _origin) = registered();
        let other = env.make_origin("other", false);
        repos::add_repo(&env.cfg, &other.to_string_lossy(), None).unwrap();
        create(&env, "origin", "alpha");
        let mut app = app(&env.cfg, TestMux::stub());

        app.panel = Panel::Repos;
        app.keys(&[KeyCode::Char('/'), KeyCode::Char('x')]);
        assert_eq!(app.repos.row_count(), 0);
        assert_eq!(
            app.contexts.row_count(),
            1,
            "other panels must keep their rows"
        );
        app.key(KeyCode::Esc);
        assert_eq!(app.repos.row_count(), 2);
        assert_eq!(app.panel, Panel::Repos);
    }

    #[test]
    fn the_ui_stays_responsive_while_statuses_fetch() {
        // A slow status provider must not stall the event loop.
        let (env, _origin) = registered();
        let cfg = slow_status_cfg(&env);
        for name in ["one", "two"] {
            contexts::create_context(&cfg, "origin", name, None).unwrap();
        }
        let mut app = app(&cfg, TestMux::stub());

        // The fetch is in flight; input must land immediately regardless.
        let start = Instant::now();
        app.key(KeyCode::Down);
        assert_eq!(app.contexts.cursor, 1);
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "input handling stalled behind the status fetch"
        );
        assert!(
            app.drain_until(|app| app.slow_cells() == ["hi", "hi"]),
            "statuses never arrived"
        );
    }

    #[test]
    fn footer_and_titles_render() {
        let (env, _origin) = registered();
        create(&env, "origin", "one");
        let mut app = app(&env.cfg, TestMux::stub());

        let text = render(&mut app);

        assert!(text.contains("[1] Contexts"));
        assert!(text.contains("[2] Repos"));
        assert!(text.contains("[3] Archived"));
        assert!(text.contains("NAME"));
        assert!(text.contains("one"));
        assert!(text.contains("Open PR"));
    }

    #[test]
    fn help_screen_lists_the_panel_bindings() {
        let (env, _origin) = registered();
        let mut app = app(&env.cfg, TestMux::stub());

        app.key(KeyCode::Char('?'));
        let text = render(&mut app);

        assert!(text.contains("Keybindings (contexts)"));
        assert!(text.contains("open the PR in the browser"));
        app.key(KeyCode::Esc);
        assert!(app.modal.is_none());
    }
}
