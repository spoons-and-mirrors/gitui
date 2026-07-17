use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
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
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    pub prefix: String,
    pub label: String,
    pub change_index: Option<usize>,
    pub directory_path: Option<String>,
}

#[derive(Default)]
struct WorktreeNode {
    children: BTreeMap<String, WorktreeNode>,
    changes: Vec<usize>,
}

pub fn build_worktree(changes: &[Change]) -> Vec<WorktreeRow> {
    let mut root = WorktreeNode::default();
    for (index, change) in changes.iter().enumerate() {
        insert_worktree_path(&mut root, &change.path, index);
    }

    let mut rows = Vec::new();
    flatten_worktree(&root, "", &[], true, &mut rows);
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
    rows: &mut Vec<WorktreeRow>,
) {
    let child_count = node.children.len();
    for (position, (name, child)) in node.children.iter().enumerate() {
        let is_last = position + 1 == child_count;
        let path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{parent_path}/{name}")
        };
        let prefix = tree_prefix(lineage, is_last, top_level);

        if child.children.is_empty() {
            for (duplicate, change_index) in child.changes.iter().enumerate() {
                rows.push(WorktreeRow {
                    prefix: if duplicate == 0 {
                        prefix.clone()
                    } else {
                        tree_prefix(lineage, is_last, top_level)
                    },
                    label: name.clone(),
                    change_index: Some(*change_index),
                    directory_path: None,
                });
            }
        } else {
            rows.push(WorktreeRow {
                prefix,
                label: name.clone(),
                change_index: None,
                directory_path: Some(path.clone()),
            });
            let mut child_lineage = lineage.to_vec();
            if !top_level {
                child_lineage.push(is_last);
            }
            flatten_worktree(child, &path, &child_lineage, false, rows);
        }
    }
}

fn tree_prefix(lineage: &[bool], is_last: bool, top_level: bool) -> String {
    if top_level {
        return String::new();
    }
    let mut prefix = String::new();
    for ancestor_is_last in lineage {
        prefix.push_str(if *ancestor_is_last { "  " } else { "│ " });
    }
    prefix.push_str(if is_last { "└─" } else { "├─" });
    prefix
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Regions {
    pub changes: Option<Rect>,
    pub graph: Option<Rect>,
    pub refresh: Option<Rect>,
    pub repository: Option<Rect>,
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
    result: Result<CommandOutput, String>,
}

pub struct App {
    pub repo: Option<RepositoryData>,
    pub view: View,
    pub mode: Mode,
    pub changes_state: ListState,
    pub graph_state: TableState,
    pub diff: String,
    pub diff_scroll: u16,
    pub commit_message: String,
    pub commit_running: bool,
    pub worktree_percent: u16,
    pub dragging_splitter: bool,
    pub picker: RepositoryPicker,
    pub notice: Option<String>,
    pub regions: Regions,
    pub should_quit: bool,
    worker_tx: Sender<WorkerResult>,
    worker_rx: Receiver<WorkerResult>,
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let (worker_tx, worker_rx) = mpsc::channel();
        let repo = git::load(&path).ok();
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
            commit_message: String::new(),
            commit_running: false,
            worktree_percent: 38,
            dragging_splitter: false,
            picker: RepositoryPicker::new(start),
            notice: None,
            regions: Regions::default(),
            should_quit: false,
            worker_tx,
            worker_rx,
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
        if self.mode == Mode::Help {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.mode = Mode::Normal;
            }
            return;
        }

        let point = Position::new(mouse.column, mouse.row);
        if self.dragging_splitter {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_worktree(mouse.column),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.resize_worktree(mouse.column);
                    self.dragging_splitter = false;
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
        } else if self.regions.help.is_some_and(|rect| rect.contains(point)) {
            self.mode = Mode::Help;
        } else if self.select_worktree_row(point) {
            if self
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
        let first_row = rect.y.saturating_add(3);
        if point.y < first_row || point.y >= rect.bottom().saturating_sub(1) {
            return false;
        }
        let index = self.graph_state.offset() + usize::from(point.y - first_row);
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
        let maximum = bounds.right().saturating_sub(24).max(minimum);
        let position = column.clamp(minimum, maximum);
        self.worktree_percent = position
            .saturating_sub(bounds.x)
            .saturating_mul(100)
            .checked_div(bounds.width.max(1))
            .unwrap_or(38)
            .clamp(15, 80);
    }

    pub fn poll_worker(&mut self) {
        let Ok(done) = self.worker_rx.try_recv() else {
            return;
        };
        self.commit_running = false;
        match done.result {
            Ok(output) => {
                if output.success {
                    self.commit_message.clear();
                    self.notice = Some("Commit created".to_owned());
                } else {
                    self.notice = Some(
                        output
                            .stderr
                            .lines()
                            .next()
                            .unwrap_or("Commit failed")
                            .to_owned(),
                    );
                }
                self.reload();
            }
            Err(error) => self.notice = Some(error),
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
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('c') if self.view == View::Changes => self.mode = Mode::Commit,
            KeyCode::Char('a') if self.view == View::Changes => self.stage_all(),
            KeyCode::Char('u') if self.view == View::Changes => self.unstage_all(),
            KeyCode::Char(' ') if self.view == View::Changes => self.toggle_stage(),
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
            KeyCode::Enter => {
                self.start_commit();
            }
            KeyCode::Backspace => {
                self.commit_message.pop();
            }
            KeyCode::Char(character) => self.commit_message.push(character),
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
                KeyCode::Char(character) => self.picker.path_input.push(character),
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
            KeyCode::Char('p') | KeyCode::Char('/') => self.picker.editing_path = true,
            KeyCode::Char('r') => self.picker.reload(),
            KeyCode::Char('q') if self.repo.is_none() => self.should_quit = true,
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
                self.repo = Some(repo);
                self.mode = Mode::Normal;
                self.notice = Some("Repository opened".to_owned());
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
            .and_then(|repo| repo.root.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.picker.directory.clone());
        self.picker.navigate(start);
        self.picker.editing_path = false;
        self.mode = Mode::Picker;
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
        let selected_oid = self
            .graph_state
            .selected()
            .and_then(|index| self.repo.as_ref()?.commits.get(index))
            .map(|commit| commit.oid.clone());

        match git::load(&root) {
            Ok(repo) => {
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
        let rows = build_worktree(&repo.changes);
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
        self.repo
            .as_ref()
            .map_or_else(Vec::new, |repo| build_worktree(&repo.changes))
    }

    fn selected_change_index(&self) -> Option<usize> {
        let selected = self.changes_state.selected()?;
        self.worktree_rows().get(selected)?.change_index
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
            let _ = sender.send(WorkerResult { result });
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
        let mut entries = vec![PickerEntry {
            label: "Open repository at this location".to_owned(),
            path: self.directory.clone(),
            action: PickerAction::Open,
            is_repo: git::discover(&self.directory).is_ok(),
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
    use super::*;

    #[test]
    fn builds_a_hierarchical_worktree_without_repeating_paths() {
        let changes = [
            change("cli/crates/sleev-tui/src/app.rs"),
            change("cli/crates/sleev-tui/src/views/home.rs"),
            change("cli/crates/sleev-tui/tests/app.rs"),
        ];

        let rows = build_worktree(&changes);
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "cli",
                "crates",
                "sleev-tui",
                "src",
                "app.rs",
                "views",
                "home.rs",
                "tests",
                "app.rs"
            ]
        );
        assert_eq!(rows[0].prefix, "");
        assert_eq!(rows[1].prefix, "└─");
        assert_eq!(rows[4].change_index, Some(0));
        assert!(rows[..4].iter().all(|row| row.change_index.is_none()));
        assert!(rows.iter().all(|row| !row.label.contains('/')));
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
