use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Position, Rect},
    widgets::{ListState, TableState},
};

use crate::git::{self, Change, CommandOutput, RepositoryData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Changes,
    Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Commit,
    Picker,
    Settings,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub auto_fetch: bool,
    pub fetch_interval_minutes: u16,
    pub worktree_width: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_fetch: false,
            fetch_interval_minutes: 5,
            worktree_width: 38,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    pub prefix: String,
    pub label: String,
    pub depth: usize,
    pub change_index: Option<usize>,
    pub directory_path: Option<String>,
    pub directory_expanded: Option<bool>,
}

#[derive(Default)]
struct WorktreeNode {
    children: BTreeMap<String, WorktreeNode>,
    changes: Vec<usize>,
}

pub fn build_worktree(changes: &[Change], collapsed: &HashSet<String>) -> Vec<WorktreeRow> {
    let mut root = WorktreeNode::default();
    for (index, change) in changes.iter().enumerate() {
        insert_worktree_path(&mut root, &change.path, index);
    }

    let mut rows = Vec::new();
    flatten_worktree(&root, "", &[], true, collapsed, &mut rows);
    rows
}

fn insert_worktree_path(root: &mut WorktreeNode, path: &str, change_index: usize) {
    let mut components = path.split('/').filter(|component| !component.is_empty());
    let mut node = root;
    for component in components.by_ref() {
        node = node.children.entry(component.to_owned()).or_default();
    }
    node.changes.push(change_index);
}

fn flatten_worktree(
    node: &WorktreeNode,
    parent_path: &str,
    lineage: &[bool],
    top_level: bool,
    collapsed: &HashSet<String>,
    rows: &mut Vec<WorktreeRow>,
) {
    let mut children: Vec<_> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| (child.children.is_empty(), name.as_str()));
    let child_count = children.len();
    for (position, (name, child)) in children.into_iter().enumerate() {
        let is_last = position + 1 == child_count;
        let first_root = top_level && position == 0;
        let mut path = join_tree_path(parent_path, name);
        let prefix = tree_prefix(lineage, is_last, first_root);

        if child.children.is_empty() {
            for (duplicate, change_index) in child.changes.iter().enumerate() {
                rows.push(WorktreeRow {
                    prefix: if duplicate == 0 {
                        prefix.clone()
                    } else {
                        tree_prefix(lineage, is_last, first_root)
                    },
                    label: name.clone(),
                    depth: lineage.len(),
                    change_index: Some(*change_index),
                    directory_path: None,
                    directory_expanded: None,
                });
            }
        } else {
            let mut label = name.clone();
            let mut directory = child;
            while directory.changes.is_empty() && directory.children.len() == 1 {
                let (next_name, next) = directory.children.first_key_value().expect("one child");
                if next.children.is_empty() {
                    break;
                }
                label.push('/');
                label.push_str(next_name);
                path = join_tree_path(&path, next_name);
                directory = next;
            }
            let expanded = !collapsed.contains(&path);
            rows.push(WorktreeRow {
                prefix,
                label,
                depth: lineage.len(),
                change_index: None,
                directory_path: Some(path.clone()),
                directory_expanded: Some(expanded),
            });
            if expanded {
                let mut child_lineage = lineage.to_vec();
                child_lineage.push(is_last);
                flatten_worktree(directory, &path, &child_lineage, false, collapsed, rows);
            }
        }
    }
}

fn join_tree_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn tree_prefix(lineage: &[bool], is_last: bool, first_root: bool) -> String {
    let mut prefix = String::from(" ");
    for ancestor_is_last in lineage {
        prefix.push_str(if *ancestor_is_last { "   " } else { "│  " });
    }
    if !first_root {
        prefix.push_str(if is_last { "└─ " } else { "├─ " });
    }
    prefix
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Regions {
    pub changes: Option<Rect>,
    pub graph: Option<Rect>,
    pub refresh: Option<Rect>,
    pub repository: Option<Rect>,
    pub settings: Option<Rect>,
    pub help: Option<Rect>,
    pub worktree: Option<Rect>,
    pub worktree_list: Option<Rect>,
    pub worktree_status: Option<Rect>,
    pub diff: Option<Rect>,
    pub splitter: Option<Rect>,
    pub split_bounds: Option<Rect>,
    pub commit: Option<Rect>,
    pub graph_table: Option<Rect>,
    pub picker_path: Option<Rect>,
    pub picker_list: Option<Rect>,
    pub picker_overlay: Option<Rect>,
    pub settings_overlay: Option<Rect>,
    pub auto_fetch: Option<Rect>,
    pub fetch_interval: Option<Rect>,
    pub fetch_interval_down: Option<Rect>,
    pub fetch_interval_up: Option<Rect>,
    pub stage_all: Option<Rect>,
    pub unstage_all: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct PickerEntry {
    pub label: String,
    pub path: PathBuf,
    pub action: PickerAction,
    pub is_repo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    Open,
    Navigate,
}

#[derive(Debug)]
pub struct RepositoryPicker {
    pub directory: PathBuf,
    pub path_input: String,
    pub editing_path: bool,
    pub entries: Vec<PickerEntry>,
    pub state: ListState,
    pub error: Option<String>,
}

#[derive(Debug)]
struct WorkerResult {
    kind: WorkerKind,
    root: PathBuf,
    result: Result<CommandOutput, String>,
}

#[derive(Debug)]
struct StatusResult {
    root: PathBuf,
    baseline: Option<u64>,
    result: Result<u64, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerKind {
    Commit,
    Fetch,
}

pub struct App {
    pub repo: Option<RepositoryData>,
    pub view: View,
    pub mode: Mode,
    pub changes_state: ListState,
    pub graph_state: TableState,
    pub diff: String,
    pub diff_scroll: u16,
    pub diff_wrap: bool,
    pub commit_message: String,
    pub commit_running: bool,
    pub collapsed_directories: HashSet<String>,
    pub dragging_splitter: bool,
    pub picker: RepositoryPicker,
    pub settings: Settings,
    pub settings_selection: usize,
    pub fetch_running: bool,
    pub notice: Option<String>,
    pub regions: Regions,
    pub should_quit: bool,
    worker_tx: Sender<WorkerResult>,
    worker_rx: Receiver<WorkerResult>,
    status_tx: Sender<StatusResult>,
    status_rx: Receiver<StatusResult>,
    status_check_running: bool,
    status_signature: Option<u64>,
    pub(crate) settings_path: Option<PathBuf>,
    next_fetch_at: Instant,
    next_status_check: Instant,
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let settings_path = settings_path();
        let settings = settings_path
            .as_deref()
            .map(load_settings)
            .unwrap_or_default();
        let next_fetch_at = Instant::now() + fetch_interval(&settings);
        let repo = git::load(&path).ok();
        let status_signature = repo
            .as_ref()
            .and_then(|repo| git::worktree_signature(&repo.root).ok());
        let mode = if repo.is_some() {
            Mode::Normal
        } else {
            Mode::Picker
        };
        let start = repo
            .as_ref()
            .and_then(|repo| repo.root.parent().map(Path::to_path_buf))
            .unwrap_or(path);

        let mut app = Self {
            repo,
            view: View::Changes,
            mode,
            changes_state: ListState::default(),
            graph_state: TableState::default(),
            diff: String::new(),
            diff_scroll: 0,
            diff_wrap: false,
            commit_message: String::new(),
            commit_running: false,
            collapsed_directories: HashSet::new(),
            dragging_splitter: false,
            picker: RepositoryPicker::new(start),
            settings,
            settings_selection: 0,
            fetch_running: false,
            notice: None,
            regions: Regions::default(),
            should_quit: false,
            worker_tx,
            worker_rx,
            status_tx,
            status_rx,
            status_check_running: false,
            status_signature,
            settings_path,
            next_fetch_at,
            next_status_check: Instant::now() + Duration::from_millis(800),
        };
        app.select_initial_rows();
        app.refresh_diff();
        app
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Commit => self.handle_commit_input(key),
            Mode::Picker => self.handle_picker(key),
            Mode::Settings => self.handle_settings(key),
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Commit => self.commit_message.push_str(text),
            Mode::Picker if self.picker.editing_path => self.picker.path_input.push_str(text),
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.mode == Mode::Picker {
            self.handle_picker_mouse(mouse);
            return;
        }
        if self.mode == Mode::Settings {
            self.handle_settings_mouse(mouse);
            return;
        }
        if self.mode == Mode::Help {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.mode = Mode::Normal;
            }
            return;
        }

        let point = Position::new(mouse.column, mouse.row);
        if self.mode == Mode::Commit
            && matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
            )
            && !self.regions.commit.is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Normal;
        }
        if self.dragging_splitter {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_worktree(mouse.column),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.resize_worktree(mouse.column);
                    self.dragging_splitter = false;
                    self.persist_settings();
                }
                _ => {}
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_at(point, 1);
                return;
            }
            MouseEventKind::ScrollUp => {
                self.scroll_at(point, -1);
                return;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.select_worktree_row(point) {
                    self.toggle_stage();
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return,
        }

        if self
            .regions
            .splitter
            .is_some_and(|rect| rect.contains(point))
        {
            self.dragging_splitter = true;
            self.resize_worktree(mouse.column);
            return;
        }
        if self
            .regions
            .stage_all
            .is_some_and(|rect| rect.contains(point))
        {
            self.toggle_all_staging();
            return;
        }
        if self
            .regions
            .unstage_all
            .is_some_and(|rect| rect.contains(point))
        {
            self.unstage_all();
            return;
        }
        if self
            .regions
            .changes
            .is_some_and(|rect| rect.contains(point))
        {
            self.view = View::Changes;
        } else if self.regions.graph.is_some_and(|rect| rect.contains(point)) {
            self.view = View::Graph;
        } else if self
            .regions
            .refresh
            .is_some_and(|rect| rect.contains(point))
        {
            self.reload();
        } else if self
            .regions
            .repository
            .is_some_and(|rect| rect.contains(point))
        {
            self.open_picker();
        } else if self
            .regions
            .settings
            .is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Settings;
        } else if self.regions.help.is_some_and(|rect| rect.contains(point)) {
            self.mode = Mode::Help;
        } else if self.select_worktree_row(point) {
            if self.selected_directory_path().is_some() {
                self.toggle_selected_directory();
            } else if self
                .regions
                .worktree_status
                .is_some_and(|rect| rect.contains(point))
            {
                self.toggle_stage();
            }
        } else if self.select_graph_row(point) {
            self.view = View::Graph;
        } else if self.regions.commit.is_some_and(|rect| rect.contains(point)) {
            self.mode = Mode::Commit;
        }
    }

    fn handle_picker_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.picker.move_selection(1),
            MouseEventKind::ScrollUp => self.picker.move_selection(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .regions
                    .picker_overlay
                    .is_some_and(|rect| !rect.contains(point))
                    && self.repo.is_some()
                {
                    self.mode = Mode::Normal;
                    return;
                }
                if self
                    .regions
                    .picker_path
                    .is_some_and(|rect| rect.contains(point))
                {
                    self.picker.editing_path = true;
                    self.picker.error = None;
                    return;
                }
                let Some(rect) = self.regions.picker_list.filter(|rect| rect.contains(point))
                else {
                    return;
                };
                let index = self.picker.state.offset() + usize::from(mouse.row - rect.y);
                if index < self.picker.entries.len() {
                    self.picker.state.select(Some(index));
                    self.activate_picker_entry(true);
                }
            }
            _ => {}
        }
    }

    fn handle_settings_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if self
            .regions
            .settings_overlay
            .is_some_and(|rect| !rect.contains(point))
        {
            self.mode = Mode::Normal;
        } else if self
            .regions
            .auto_fetch
            .is_some_and(|rect| rect.contains(point))
        {
            self.settings_selection = 0;
            self.toggle_auto_fetch();
        } else if self
            .regions
            .fetch_interval_down
            .is_some_and(|rect| rect.contains(point))
        {
            self.settings_selection = 1;
            self.change_fetch_interval(-1);
        } else if self
            .regions
            .fetch_interval_up
            .is_some_and(|rect| rect.contains(point))
        {
            self.settings_selection = 1;
            self.change_fetch_interval(1);
        } else if self
            .regions
            .fetch_interval
            .is_some_and(|rect| rect.contains(point))
        {
            self.settings_selection = 1;
        }
    }

    fn select_worktree_row(&mut self, point: Position) -> bool {
        if self.view != View::Changes {
            return false;
        }
        let Some(rect) = self
            .regions
            .worktree_list
            .filter(|rect| rect.contains(point))
        else {
            return false;
        };
        let index = self.changes_state.offset() + usize::from(point.y - rect.y);
        let len = self.worktree_rows().len();
        if index >= len {
            return false;
        }
        self.changes_state.select(Some(index));
        self.refresh_diff();
        true
    }

    fn select_graph_row(&mut self, point: Position) -> bool {
        if self.view != View::Graph {
            return false;
        }
        let Some(rect) = self.regions.graph_table.filter(|rect| rect.contains(point)) else {
            return false;
        };
        let index = self.graph_state.offset() + usize::from(point.y - rect.y);
        let len = self.repo.as_ref().map_or(0, |repo| repo.commits.len());
        if index >= len {
            return false;
        }
        self.graph_state.select(Some(index));
        true
    }

    fn scroll_at(&mut self, point: Position, delta: isize) {
        if self.regions.diff.is_some_and(|rect| rect.contains(point)) {
            self.diff_scroll = if delta > 0 {
                self.diff_scroll.saturating_add(3)
            } else {
                self.diff_scroll.saturating_sub(3)
            };
        } else if self
            .regions
            .worktree
            .is_some_and(|rect| rect.contains(point))
            || self
                .regions
                .graph_table
                .is_some_and(|rect| rect.contains(point))
        {
            self.move_selection(delta);
        }
    }

    fn resize_worktree(&mut self, column: u16) {
        let Some(bounds) = self.regions.split_bounds else {
            return;
        };
        let minimum = bounds.x.saturating_add(24);
        let maximum = bounds.right().saturating_sub(25).max(minimum);
        let position = column.clamp(minimum, maximum);
        self.settings.worktree_width = position.saturating_sub(bounds.x);
    }

    pub fn poll_worker(&mut self) {
        self.maybe_start_auto_fetch();
        self.maybe_check_worktree();
        while let Ok(done) = self.worker_rx.try_recv() {
            let active_repository = self
                .repo
                .as_ref()
                .is_some_and(|repo| repo.root == done.root);
            match done.kind {
                WorkerKind::Commit => {
                    self.commit_running = false;
                    if !active_repository {
                        continue;
                    }
                    match done.result {
                        Ok(output) if output.success => {
                            self.commit_message.clear();
                            self.reload();
                            self.notice = Some("Commit created".to_owned());
                        }
                        Ok(output) => {
                            self.notice = Some(first_error(&output.stderr, "Commit failed"));
                        }
                        Err(error) => self.notice = Some(error),
                    }
                }
                WorkerKind::Fetch => {
                    self.fetch_running = false;
                    self.next_fetch_at = Instant::now() + fetch_interval(&self.settings);
                    if !active_repository {
                        continue;
                    }
                    match done.result {
                        Ok(output) if output.success => {
                            self.reload();
                            self.notice = Some("Fetched remotes".to_owned());
                        }
                        Ok(output) => {
                            self.notice = Some(first_error(&output.stderr, "Fetch failed"));
                        }
                        Err(error) => self.notice = Some(error),
                    }
                }
            }
        }
        while let Ok(done) = self.status_rx.try_recv() {
            self.status_check_running = false;
            let active_repository = self
                .repo
                .as_ref()
                .is_some_and(|repo| repo.root == done.root);
            if !active_repository || self.status_signature != done.baseline {
                continue;
            }
            if let Ok(signature) = done.result {
                let changed = self
                    .status_signature
                    .replace(signature)
                    .is_some_and(|previous| previous != signature);
                if changed {
                    self.reload();
                    if self.notice.as_deref() == Some("Refreshed") {
                        self.notice = None;
                    }
                }
            }
        }
    }

    pub fn change_counts(&self) -> (usize, usize) {
        self.repo.as_ref().map_or((0, 0), |repo| {
            (
                repo.changes.iter().filter(|change| change.staged).count(),
                repo.changes.iter().filter(|change| !change.staged).count(),
            )
        })
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') if self.commit_running => {
                self.notice = Some("A commit is still running".to_owned())
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => self.view = View::Changes,
            KeyCode::Char('2') => self.view = View::Graph,
            KeyCode::Tab => {
                self.view = match self.view {
                    View::Changes => View::Graph,
                    View::Graph => View::Changes,
                }
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('o') => self.open_picker(),
            KeyCode::Char('s') => self.mode = Mode::Settings,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('w') if self.view == View::Changes => {
                self.diff_wrap = !self.diff_wrap;
                self.diff_scroll = 0;
                self.notice = Some(
                    if self.diff_wrap {
                        "Diff wrap enabled"
                    } else {
                        "Diff wrap disabled"
                    }
                    .to_owned(),
                );
            }
            KeyCode::Char('c') if self.view == View::Changes => self.mode = Mode::Commit,
            KeyCode::Char('a') if self.view == View::Changes => self.stage_all(),
            KeyCode::Char('u') if self.view == View::Changes => self.unstage_all(),
            KeyCode::Char(' ') if self.view == View::Changes => self.toggle_stage(),
            KeyCode::Enter if self.view == View::Changes => self.toggle_selected_directory(),
            KeyCode::Right | KeyCode::Char('l') if self.view == View::Changes => {
                self.expand_or_descend_worktree()
            }
            KeyCode::Left | KeyCode::Char('h') if self.view == View::Changes => {
                self.collapse_or_ascend_worktree()
            }
            KeyCode::PageDown if self.view == View::Changes => {
                self.diff_scroll = self.diff_scroll.saturating_add(10)
            }
            KeyCode::PageUp if self.view == View::Changes => {
                self.diff_scroll = self.diff_scroll.saturating_sub(10)
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Home | KeyCode::Char('g') => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_last(),
            _ => {}
        }
    }

    fn handle_commit_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_commit();
            }
            KeyCode::Char('j' | 'm') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_commit();
            }
            KeyCode::Enter => self.commit_message.push('\n'),
            KeyCode::Backspace => {
                self.commit_message.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.commit_message.push(character);
            }
            _ => {}
        }
    }

    fn handle_picker(&mut self, key: KeyEvent) {
        if self.picker.editing_path {
            match key.code {
                KeyCode::Esc => self.picker.editing_path = false,
                KeyCode::Enter => {
                    let path = self.picker.input_path();
                    self.open_repository(path);
                }
                KeyCode::Backspace => {
                    self.picker.path_input.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.picker.path_input.clear();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.picker.path_input.push(character);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc if self.repo.is_some() => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => self.picker.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.picker.move_selection(-1),
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.picker.go_parent(),
            KeyCode::Enter => self.activate_picker_entry(true),
            KeyCode::Right | KeyCode::Char('l') => self.activate_picker_entry(false),
            KeyCode::Char('p') | KeyCode::Char('/') => {
                self.picker.editing_path = true;
                self.picker.error = None;
            }
            KeyCode::Char('r') => self.picker.reload(),
            KeyCode::Char('q') if self.repo.is_none() => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_settings(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('s') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.settings_selection = (self.settings_selection + 1) % 2;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.settings_selection = (self.settings_selection + 1) % 2;
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.settings_selection == 0 => {
                self.toggle_auto_fetch();
            }
            KeyCode::Left | KeyCode::Char('-') if self.settings_selection == 1 => {
                self.change_fetch_interval(-1);
            }
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=')
                if self.settings_selection == 1 =>
            {
                self.change_fetch_interval(1);
            }
            _ => {}
        }
    }

    fn activate_picker_entry(&mut self, open_repositories: bool) {
        let Some(entry) = self.picker.selected().cloned() else {
            return;
        };
        if open_repositories && entry.action == PickerAction::Navigate && entry.is_repo {
            self.open_repository(entry.path);
            return;
        }
        match entry.action {
            PickerAction::Navigate => self.picker.navigate(entry.path),
            PickerAction::Open => self.open_repository(entry.path),
        }
    }

    fn open_repository(&mut self, path: PathBuf) {
        match git::load(&path) {
            Ok(repo) => {
                self.status_signature = git::worktree_signature(&repo.root).ok();
                self.next_status_check = Instant::now() + Duration::from_millis(800);
                self.repo = Some(repo);
                self.mode = Mode::Normal;
                self.notice = Some("Repository opened".to_owned());
                self.next_fetch_at = Instant::now() + fetch_interval(&self.settings);
                self.collapsed_directories.clear();
                self.changes_state = ListState::default();
                self.graph_state = TableState::default();
                self.select_initial_rows();
                self.refresh_diff();
            }
            Err(error) => self.picker.error = Some(error.to_string()),
        }
    }

    fn open_picker(&mut self) {
        let start = self
            .repo
            .as_ref()
            .map(|repo| repo.root.clone())
            .unwrap_or_else(|| self.picker.directory.clone());
        self.picker.navigate(start);
        self.picker.editing_path = false;
        self.mode = Mode::Picker;
    }

    fn toggle_auto_fetch(&mut self) {
        self.settings.auto_fetch = !self.settings.auto_fetch;
        self.settings_changed();
    }

    fn change_fetch_interval(&mut self, delta: i16) {
        self.settings.fetch_interval_minutes =
            (self.settings.fetch_interval_minutes as i16 + delta).clamp(1, 1440) as u16;
        self.settings_changed();
    }

    fn settings_changed(&mut self) {
        self.next_fetch_at = Instant::now() + fetch_interval(&self.settings);
        self.persist_settings();
    }

    fn persist_settings(&mut self) {
        if let Some(path) = &self.settings_path
            && let Err(error) = save_settings(path, &self.settings)
        {
            self.notice = Some(format!("Could not save settings: {error}"));
        }
    }

    fn maybe_start_auto_fetch(&mut self) {
        if !self.settings.auto_fetch || self.fetch_running || Instant::now() < self.next_fetch_at {
            return;
        }
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        self.fetch_running = true;
        self.next_fetch_at = Instant::now() + fetch_interval(&self.settings);
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = git::fetch(&root).map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult {
                kind: WorkerKind::Fetch,
                root,
                result,
            });
        });
    }

    fn maybe_check_worktree(&mut self) {
        if self.status_check_running
            || self.commit_running
            || self.fetch_running
            || Instant::now() < self.next_status_check
        {
            return;
        }
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        self.status_check_running = true;
        self.next_status_check = Instant::now() + Duration::from_millis(800);
        let baseline = self.status_signature;
        let sender = self.status_tx.clone();
        thread::spawn(move || {
            let result = git::worktree_signature(&root).map_err(|error| error.to_string());
            let _ = sender.send(StatusResult {
                root,
                baseline,
                result,
            });
        });
    }

    fn move_selection(&mut self, delta: isize) {
        match self.view {
            View::Changes => {
                let len = self.worktree_rows().len();
                move_list(&mut self.changes_state, len, delta);
                self.refresh_diff();
            }
            View::Graph => {
                let len = self.repo.as_ref().map_or(0, |repo| repo.commits.len());
                move_list(&mut self.graph_state, len, delta);
            }
        }
    }

    fn select_first(&mut self) {
        match self.view {
            View::Changes => {
                self.changes_state.select(self.first_change_row());
                self.refresh_diff();
            }
            View::Graph => self.graph_state.select(
                self.repo
                    .as_ref()
                    .is_some_and(|repo| !repo.commits.is_empty())
                    .then_some(0),
            ),
        }
    }

    fn select_last(&mut self) {
        match self.view {
            View::Changes => {
                self.changes_state.select(self.last_change_row());
                self.refresh_diff();
            }
            View::Graph => self.graph_state.select(
                self.repo
                    .as_ref()
                    .and_then(|repo| repo.commits.len().checked_sub(1)),
            ),
        }
    }

    fn toggle_stage(&mut self) {
        let Some(repo) = &self.repo else { return };
        let Some(index) = self.selected_change_index() else {
            return;
        };
        let Some(change) = repo.changes.get(index).cloned() else {
            return;
        };
        let result = if change.staged {
            git::unstage(&repo.root, &change)
        } else {
            git::stage(&repo.root, &change)
        };
        if let Err(error) = result {
            self.notice = Some(error.to_string());
        } else {
            self.reload();
        }
    }

    fn stage_all(&mut self) {
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        if let Err(error) = git::stage_all(&root) {
            self.notice = Some(error.to_string());
        } else {
            self.reload();
        }
    }

    fn toggle_all_staging(&mut self) {
        let all_staged = self.repo.as_ref().is_some_and(|repo| {
            !repo.changes.is_empty() && repo.changes.iter().all(|change| change.staged)
        });
        if all_staged {
            self.unstage_all();
        } else {
            self.stage_all();
        }
    }

    fn unstage_all(&mut self) {
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        if let Err(error) = git::unstage_all(&root) {
            self.notice = Some(error.to_string());
        } else {
            self.reload();
        }
    }

    fn reload(&mut self) {
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        let selected_change = self
            .selected_change_index()
            .and_then(|index| self.repo.as_ref()?.changes.get(index))
            .map(|change| (change.path.clone(), change.staged));
        let selected_directory = self.selected_directory_path();
        let selected_oid = self
            .graph_state
            .selected()
            .and_then(|index| self.repo.as_ref()?.commits.get(index))
            .map(|commit| commit.oid.clone());

        match git::load(&root) {
            Ok(repo) => {
                self.status_signature = git::worktree_signature(&repo.root).ok();
                let change_index = selected_change.and_then(|(path, staged)| {
                    repo.changes
                        .iter()
                        .position(|change| change.path == path && change.staged == staged)
                        .or_else(|| repo.changes.iter().position(|change| change.path == path))
                });
                let commit_index = selected_oid
                    .and_then(|oid| repo.commits.iter().position(|commit| commit.oid == oid));
                self.repo = Some(repo);
                let change_row = change_index
                    .and_then(|index| self.row_for_change(index))
                    .or_else(|| {
                        let directory = selected_directory.as_ref()?;
                        self.worktree_rows()
                            .iter()
                            .position(|row| row.directory_path.as_ref() == Some(directory))
                    })
                    .or_else(|| self.first_change_row());
                self.changes_state.select(change_row);
                self.graph_state.select(
                    commit_index.or_else(|| self.repo.as_ref()?.commits.first().map(|_| 0)),
                );
                self.notice = Some("Refreshed".to_owned());
                self.refresh_diff();
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    fn refresh_diff(&mut self) {
        self.diff_scroll = 0;
        let Some(repo) = &self.repo else {
            self.diff.clear();
            return;
        };
        let rows = build_worktree(&repo.changes, &self.collapsed_directories);
        let Some(row) = self
            .changes_state
            .selected()
            .and_then(|index| rows.get(index))
        else {
            self.diff = "Working tree clean".to_owned();
            return;
        };
        if let Some(index) = row.change_index {
            self.diff = git::diff(&repo.root, &repo.changes[index])
                .unwrap_or_else(|error| error.to_string());
        } else if let Some(path) = &row.directory_path {
            let count = repo
                .changes
                .iter()
                .filter(|change| change.path.starts_with(&format!("{path}/")))
                .count();
            self.diff = format!("{count} changed files in {path}/");
        }
    }

    fn select_initial_rows(&mut self) {
        self.changes_state.select(self.first_change_row());
        self.graph_state.select(
            self.repo
                .as_ref()
                .is_some_and(|repo| !repo.commits.is_empty())
                .then_some(0),
        );
    }

    pub fn worktree_rows(&self) -> Vec<WorktreeRow> {
        self.repo.as_ref().map_or_else(Vec::new, |repo| {
            build_worktree(&repo.changes, &self.collapsed_directories)
        })
    }

    fn selected_change_index(&self) -> Option<usize> {
        let selected = self.changes_state.selected()?;
        self.worktree_rows().get(selected)?.change_index
    }

    fn selected_directory_path(&self) -> Option<String> {
        let selected = self.changes_state.selected()?;
        self.worktree_rows().get(selected)?.directory_path.clone()
    }

    fn toggle_selected_directory(&mut self) {
        let Some(path) = self.selected_directory_path() else {
            return;
        };
        if !self.collapsed_directories.remove(&path) {
            self.collapsed_directories.insert(path.clone());
        }
        self.select_directory(&path);
        self.refresh_diff();
    }

    fn expand_or_descend_worktree(&mut self) {
        let rows = self.worktree_rows();
        let Some(index) = self.changes_state.selected() else {
            return;
        };
        let Some(row) = rows.get(index) else { return };
        let Some(path) = &row.directory_path else {
            return;
        };
        if row.directory_expanded == Some(false) {
            self.collapsed_directories.remove(path);
            self.select_directory(path);
        } else if rows
            .get(index + 1)
            .is_some_and(|child| child.depth > row.depth)
        {
            self.changes_state.select(Some(index + 1));
        }
        self.refresh_diff();
    }

    fn collapse_or_ascend_worktree(&mut self) {
        let rows = self.worktree_rows();
        let Some(index) = self.changes_state.selected() else {
            return;
        };
        let Some(row) = rows.get(index) else { return };
        if let Some(path) = &row.directory_path
            && row.directory_expanded == Some(true)
        {
            self.collapsed_directories.insert(path.clone());
            self.select_directory(path);
            self.refresh_diff();
            return;
        }
        if let Some(parent) = rows[..index]
            .iter()
            .rposition(|candidate| candidate.depth < row.depth)
        {
            self.changes_state.select(Some(parent));
            self.refresh_diff();
        }
    }

    fn select_directory(&mut self, path: &str) {
        let row = self
            .worktree_rows()
            .iter()
            .position(|row| row.directory_path.as_deref() == Some(path));
        self.changes_state.select(row);
    }

    fn row_for_change(&self, change_index: usize) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.change_index == Some(change_index))
    }

    fn first_change_row(&self) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.change_index.is_some())
    }

    fn last_change_row(&self) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .rposition(|row| row.change_index.is_some())
    }

    fn start_commit(&mut self) {
        if self.commit_running {
            self.notice = Some("A commit is already running".to_owned());
            return;
        }
        let message = self.commit_message.trim().to_owned();
        if message.is_empty() {
            self.notice = Some("Commit message cannot be empty".to_owned());
            return;
        }
        let Some(root) = self.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        self.commit_running = true;
        self.mode = Mode::Normal;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = git::commit(&root, &message).map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult {
                kind: WorkerKind::Commit,
                root,
                result,
            });
        });
    }
}

impl RepositoryPicker {
    fn new(directory: PathBuf) -> Self {
        let mut picker = Self {
            path_input: directory.display().to_string(),
            directory,
            editing_path: false,
            entries: Vec::new(),
            state: ListState::default(),
            error: None,
        };
        picker.reload();
        picker
    }

    fn reload(&mut self) {
        self.error = None;
        let current_is_repo = git::discover(&self.directory).is_ok();
        let mut entries = vec![PickerEntry {
            label: if current_is_repo {
                "Open current repository".to_owned()
            } else {
                "Open current location".to_owned()
            },
            path: self.directory.clone(),
            action: PickerAction::Open,
            is_repo: current_is_repo,
        }];

        if let Some(parent) = self.directory.parent() {
            entries.push(PickerEntry {
                label: "..".to_owned(),
                path: parent.to_path_buf(),
                action: PickerAction::Navigate,
                is_repo: false,
            });
        }

        match fs::read_dir(&self.directory) {
            Ok(read_dir) => {
                let mut directories: Vec<_> = read_dir
                    .filter_map(Result::ok)
                    .filter_map(|entry| {
                        let file_type = entry.file_type().ok()?;
                        (file_type.is_dir() || file_type.is_symlink()).then_some(entry)
                    })
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .map(|entry| {
                        let path = entry.path();
                        let is_repo = path.join(".git").exists();
                        PickerEntry {
                            label: format!("{}/", entry.file_name().to_string_lossy()),
                            path,
                            action: PickerAction::Navigate,
                            is_repo,
                        }
                    })
                    .collect();
                directories.sort_by_key(|entry| entry.label.to_lowercase());
                entries.extend(directories);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        self.entries = entries;
        self.state.select((!self.entries.is_empty()).then_some(0));
    }

    fn selected(&self) -> Option<&PickerEntry> {
        self.state
            .selected()
            .and_then(|index| self.entries.get(index))
    }

    fn move_selection(&mut self, delta: isize) {
        move_list(&mut self.state, self.entries.len(), delta);
    }

    fn navigate(&mut self, path: PathBuf) {
        self.directory = path;
        self.path_input = self.directory.display().to_string();
        self.reload();
    }

    fn go_parent(&mut self) {
        if let Some(parent) = self.directory.parent() {
            self.navigate(parent.to_path_buf());
        }
    }

    fn input_path(&self) -> PathBuf {
        let input = self.path_input.trim();
        let expanded = if input == "~" {
            home_directory().unwrap_or_else(|| PathBuf::from(input))
        } else if let Some(rest) = input.strip_prefix("~/") {
            home_directory()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(input))
        } else {
            PathBuf::from(input)
        };
        if expanded.is_absolute() {
            expanded
        } else {
            self.directory.join(expanded)
        }
    }
}

fn fetch_interval(settings: &Settings) -> Duration {
    Duration::from_secs(u64::from(settings.fetch_interval_minutes) * 60)
}

fn settings_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join("gitui/config"));
    }
    if let Some(path) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(path).join("gitui/config"));
    }
    home_directory().map(|home| home.join(".config/gitui/config"))
}

fn load_settings(path: &Path) -> Settings {
    let Ok(contents) = fs::read_to_string(path) else {
        return Settings::default();
    };
    let mut settings = Settings::default();
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "auto_fetch" => settings.auto_fetch = value.trim() == "true",
            "fetch_interval_minutes" => {
                if let Ok(minutes) = value.trim().parse::<u16>() {
                    settings.fetch_interval_minutes = minutes.clamp(1, 1440);
                }
            }
            "worktree_width" => {
                if let Ok(width) = value.trim().parse::<u16>() {
                    settings.worktree_width = width.clamp(24, 4096);
                }
            }
            _ => {}
        }
    }
    settings
}

fn save_settings(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!(
            "auto_fetch={}\nfetch_interval_minutes={}\nworktree_width={}\n",
            settings.auto_fetch, settings.fetch_interval_minutes, settings.worktree_width
        ),
    )
}

fn first_error(stderr: &str, fallback: &str) -> String {
    stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn move_list<S: SelectionState>(state: &mut S, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0);
    let next = (current as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
    state.select(Some(next));
}

trait SelectionState {
    fn selected(&self) -> Option<usize>;
    fn select(&mut self, index: Option<usize>);
}

impl SelectionState for ListState {
    fn selected(&self) -> Option<usize> {
        ListState::selected(self)
    }

    fn select(&mut self, index: Option<usize>) {
        ListState::select(self, index);
    }
}

impl SelectionState for TableState {
    fn selected(&self) -> Option<usize> {
        TableState::selected(self)
    }

    fn select(&mut self, index: Option<usize>) {
        TableState::select(self, index);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn builds_a_hierarchical_worktree_without_repeating_paths() {
        let changes = [
            change("cli/crates/sleev-tui/src/app.rs"),
            change("cli/crates/sleev-tui/src/views/home.rs"),
            change("cli/crates/sleev-tui/tests/app.rs"),
        ];

        let rows = build_worktree(&changes, &HashSet::new());
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "cli/crates/sleev-tui",
                "src",
                "views",
                "home.rs",
                "app.rs",
                "tests",
                "app.rs"
            ]
        );
        assert_eq!(rows[0].prefix, " ");
        assert_eq!(rows[1].prefix, "    ├─ ");
        assert_eq!(rows[2].label, "views");
        assert_eq!(rows[4].change_index, Some(0));
        assert_eq!(rows[3].change_index, Some(1));
        assert_eq!(rows[6].change_index, Some(2));

        let collapsed = HashSet::from(["cli/crates/sleev-tui".to_owned()]);
        let rows = build_worktree(&changes, &collapsed);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].directory_expanded, Some(false));
    }

    #[test]
    fn persists_auto_fetch_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config");
        let settings = Settings {
            auto_fetch: true,
            fetch_interval_minutes: 17,
            worktree_width: 61,
        };

        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);

        fs::write(
            &path,
            "auto_fetch=true\nfetch_interval_minutes=0\nworktree_width=5\n",
        )
        .unwrap();
        let loaded = load_settings(&path);
        assert_eq!(loaded.fetch_interval_minutes, 1);
        assert_eq!(loaded.worktree_width, 24);
    }

    #[test]
    fn switches_views_with_tab_and_edits_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config");
        let mut app = App::new(directory.path().join("missing"));
        app.mode = Mode::Normal;
        app.settings = Settings::default();
        app.settings_path = Some(path.clone());

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.view, View::Graph);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.view, View::Changes);

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Settings);
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.settings,
            Settings {
                auto_fetch: true,
                fetch_interval_minutes: 6,
                worktree_width: 38,
            }
        );
        assert_eq!(load_settings(&path), app.settings);

        app.mode = Mode::Normal;
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(app.diff_wrap);
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(!app.diff_wrap);
    }

    #[test]
    fn auto_fetch_runs_without_blocking_the_app() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.name", "Fetch Test"][..],
            &["config", "user.email", "fetch@example.com"][..],
        ] {
            let output = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        for args in [&["add", "."][..], &["commit", "-m", "initial"][..]] {
            let output = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        }

        let mut app = App::new(root.to_path_buf());
        app.settings.auto_fetch = true;
        app.next_fetch_at = Instant::now();
        app.poll_worker();
        assert!(app.fetch_running);

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.fetch_running {
                break;
            }
        }
        assert!(!app.fetch_running);
        assert_eq!(app.notice.as_deref(), Some("Fetched remotes"));
    }

    #[test]
    fn control_j_commits_in_terminals_that_encode_control_enter_as_line_feed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        fs::write(root.join("next.txt"), "next\n").unwrap();
        run_git(root, &["add", "next.txt"]);

        let mut app = App::new(root.to_path_buf());
        app.mode = Mode::Commit;
        app.commit_message = "commit from control enter".to_owned();
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(app.commit_running);
        assert_eq!(app.commit_message, "commit from control enter");

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.commit_running {
                break;
            }
        }
        assert!(!app.commit_running);
        assert!(app.commit_message.is_empty());
        assert_eq!(app.repo.as_ref().unwrap().commits.len(), 2);
    }

    #[test]
    fn refreshes_an_already_dirty_file_when_its_contents_change_again() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let tracked = root.join("tracked.txt");
        let mut app = App::new(root.to_path_buf());

        fs::write(&tracked, "first\n").unwrap();
        app.next_status_check = Instant::now();
        for _ in 0..100 {
            app.poll_worker();
            if app.diff.contains("first") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.diff.contains("first"));

        fs::write(&tracked, "later\n").unwrap();
        app.next_status_check = Instant::now();
        for _ in 0..100 {
            app.poll_worker();
            if app.diff.contains("later") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.diff.contains("later"));
    }

    fn initialize_repository(root: &Path) {
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.name", "App Test"][..],
            &["config", "user.email", "app@example.com"][..],
        ] {
            run_git(root, args);
        }
        fs::write(root.join("tracked.txt"), "base\n").unwrap();
        run_git(root, &["add", "tracked.txt"]);
        run_git(root, &["commit", "-m", "initial"]);
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.to_owned(),
            original_path: None,
            code: 'M',
            staged: false,
        }
    }
}
