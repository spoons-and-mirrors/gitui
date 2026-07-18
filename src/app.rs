use std::{
    collections::{BTreeMap, HashSet, VecDeque},
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
pub enum LeftPane {
    Worktree,
    Files,
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
    pub history_height: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_fetch: false,
            fetch_interval_minutes: 5,
            worktree_width: 38,
            history_height: 7,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerRow {
    pub prefix: String,
    pub label: String,
    pub depth: usize,
    pub file_index: Option<usize>,
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

pub fn build_file_tree(files: &[String], collapsed: &HashSet<String>) -> Vec<ExplorerRow> {
    let mut root = WorktreeNode::default();
    for (index, path) in files.iter().enumerate() {
        insert_worktree_path(&mut root, path, index);
    }
    let mut rows = Vec::new();
    flatten_file_tree(&root, "", &[], true, collapsed, &mut rows);
    rows
}

fn flatten_file_tree(
    node: &WorktreeNode,
    parent_path: &str,
    lineage: &[bool],
    top_level: bool,
    collapsed: &HashSet<String>,
    rows: &mut Vec<ExplorerRow>,
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
            if let Some(file_index) = child.changes.first() {
                rows.push(ExplorerRow {
                    prefix,
                    label: name.clone(),
                    depth: lineage.len(),
                    file_index: Some(*file_index),
                    directory_path: None,
                    directory_expanded: None,
                });
            }
            continue;
        }

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
        rows.push(ExplorerRow {
            prefix,
            label,
            depth: lineage.len(),
            file_index: None,
            directory_path: Some(path.clone()),
            directory_expanded: Some(expanded),
        });
        if expanded {
            let mut child_lineage = lineage.to_vec();
            child_lineage.push(is_last);
            flatten_file_tree(directory, &path, &child_lineage, false, collapsed, rows);
        }
    }
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
    pub worktree_tab: Option<Rect>,
    pub files_tab: Option<Rect>,
    pub worktree_list: Option<Rect>,
    pub explorer_list: Option<Rect>,
    pub worktree_status: Option<Rect>,
    pub history_list: Option<Rect>,
    pub history_splitter: Option<Rect>,
    pub history_bounds: Option<Rect>,
    pub diff: Option<Rect>,
    pub diff_scrollbar: Option<Rect>,
    pub diff_scroll_thumb: Option<Rect>,
    pub diff_scroll_max: u16,
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
    pub matches: Vec<PickerEntry>,
    pub match_state: ListState,
    pub searching: bool,
    pub error: Option<String>,
    directory_index: Vec<PathBuf>,
    index_rx: Option<Receiver<Vec<PathBuf>>>,
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
    pub left_pane: LeftPane,
    pub mode: Mode,
    pub changes_state: ListState,
    pub explorer_state: ListState,
    pub graph_state: TableState,
    pub history_state: ListState,
    pub diff: String,
    pub diff_scroll: u16,
    pub diff_wrap: bool,
    pub commit_message: String,
    pub commit_running: bool,
    pub collapsed_directories: HashSet<String>,
    pub collapsed_explorer_directories: HashSet<String>,
    pub dragging_splitter: bool,
    pub dragging_history: bool,
    pub dragging_diff_scrollbar: bool,
    diff_scroll_drag_offset: u16,
    pub history_focused: bool,
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
            left_pane: LeftPane::Worktree,
            mode,
            changes_state: ListState::default(),
            explorer_state: ListState::default(),
            graph_state: TableState::default(),
            history_state: ListState::default(),
            diff: String::new(),
            diff_scroll: 0,
            diff_wrap: false,
            commit_message: String::new(),
            commit_running: false,
            collapsed_directories: HashSet::new(),
            collapsed_explorer_directories: HashSet::new(),
            dragging_splitter: false,
            dragging_history: false,
            dragging_diff_scrollbar: false,
            diff_scroll_drag_offset: 0,
            history_focused: false,
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
            Mode::Picker if self.picker.editing_path => {
                self.picker.path_input.push_str(text);
                self.picker.refresh_matches();
            }
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
        if self.dragging_history {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_history(mouse.row),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.resize_history(mouse.row);
                    self.dragging_history = false;
                    self.persist_settings();
                }
                _ => {}
            }
            return;
        }
        if self.dragging_diff_scrollbar {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.scroll_diff_to(mouse.row),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.scroll_diff_to(mouse.row);
                    self.dragging_diff_scrollbar = false;
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
            .history_splitter
            .is_some_and(|rect| rect.contains(point))
        {
            self.dragging_history = true;
            self.history_focused = true;
            self.resize_history(mouse.row);
            return;
        }
        if self
            .regions
            .diff_scrollbar
            .is_some_and(|rect| rect.contains(point))
            && self.regions.diff_scroll_max > 0
        {
            self.dragging_diff_scrollbar = true;
            self.diff_scroll_drag_offset = self
                .regions
                .diff_scroll_thumb
                .filter(|thumb| thumb.contains(point))
                .map_or_else(
                    || {
                        self.regions
                            .diff_scroll_thumb
                            .map_or(0, |thumb| thumb.height / 2)
                    },
                    |thumb| point.y.saturating_sub(thumb.y),
                );
            self.scroll_diff_to(mouse.row);
            return;
        }
        if self
            .regions
            .worktree_tab
            .is_some_and(|rect| rect.contains(point))
        {
            self.set_left_pane(LeftPane::Worktree);
            return;
        }
        if self
            .regions
            .files_tab
            .is_some_and(|rect| rect.contains(point))
        {
            self.set_left_pane(LeftPane::Files);
            return;
        }
        if self
            .regions
            .stage_all
            .is_some_and(|rect| rect.contains(point))
        {
            self.clear_history_selection();
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
        } else if self.select_explorer_row(point) {
            if self.selected_explorer_directory_path().is_some() {
                self.toggle_selected_explorer_directory();
            }
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
        } else if self
            .regions
            .worktree_list
            .is_some_and(|rect| rect.contains(point))
        {
            self.clear_history_selection();
            self.refresh_diff();
        } else if self.select_history_row(point) {
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
                    self.picker.begin_search(None);
                    return;
                }
                let Some(rect) = self.regions.picker_list.filter(|rect| rect.contains(point))
                else {
                    return;
                };
                let index = self.picker.state.offset() + usize::from(mouse.row - rect.y);
                if self.picker.editing_path {
                    let index = self.picker.match_state.offset() + usize::from(mouse.row - rect.y);
                    if index < self.picker.matches.len() {
                        self.picker.match_state.select(Some(index));
                        self.confirm_picker_path();
                    }
                } else if index < self.picker.entries.len() {
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
        if self.view != View::Changes || self.left_pane != LeftPane::Worktree {
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
        self.clear_history_selection();
        self.refresh_diff();
        true
    }

    fn select_explorer_row(&mut self, point: Position) -> bool {
        if self.view != View::Changes || self.left_pane != LeftPane::Files {
            return false;
        }
        let Some(rect) = self
            .regions
            .explorer_list
            .filter(|rect| rect.contains(point))
        else {
            return false;
        };
        let index = self.explorer_state.offset() + usize::from(point.y - rect.y);
        if index >= self.explorer_rows().len() {
            return false;
        }
        self.explorer_state.select(Some(index));
        self.refresh_diff();
        true
    }

    fn select_history_row(&mut self, point: Position) -> bool {
        if self.view != View::Changes {
            return false;
        }
        let Some(rect) = self
            .regions
            .history_list
            .filter(|rect| rect.contains(point))
        else {
            return false;
        };
        let Some(repo) = self.repo.as_ref() else {
            return false;
        };
        let relative_row = usize::from(point.y - rect.y);
        let mut rendered_row = 0;
        let index = (self.history_state.offset()..repo.history.len()).find(|index| {
            let height = if repo.history[*index].refs.is_empty() {
                1
            } else {
                2
            };
            let contains = relative_row < rendered_row + height;
            rendered_row += height;
            contains
        });
        let Some(index) = index else {
            return false;
        };
        self.history_state.select(Some(index));
        self.history_focused = true;
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
            self.scroll_diff_by(delta.saturating_mul(3));
        } else if self
            .regions
            .explorer_list
            .is_some_and(|rect| rect.contains(point))
        {
            self.move_explorer_selection(delta);
        } else if self
            .regions
            .history_list
            .is_some_and(|rect| rect.contains(point))
        {
            self.history_focused = true;
            let len = self.repo.as_ref().map_or(0, |repo| repo.history.len());
            move_list(&mut self.history_state, len, delta);
            self.refresh_diff();
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

    fn scroll_diff_by(&mut self, delta: isize) {
        self.diff_scroll = if delta > 0 {
            self.diff_scroll
                .saturating_add(delta as u16)
                .min(self.regions.diff_scroll_max)
        } else {
            self.diff_scroll.saturating_sub(delta.unsigned_abs() as u16)
        };
    }

    fn scroll_diff_to(&mut self, row: u16) {
        let Some(track) = self.regions.diff_scrollbar else {
            return;
        };
        let Some(thumb) = self.regions.diff_scroll_thumb else {
            return;
        };
        let travel = track.height.saturating_sub(thumb.height);
        if travel == 0 || self.regions.diff_scroll_max == 0 {
            self.diff_scroll = 0;
            return;
        }
        let position = row
            .saturating_sub(track.y)
            .saturating_sub(self.diff_scroll_drag_offset)
            .min(travel);
        self.diff_scroll = ((u32::from(position) * u32::from(self.regions.diff_scroll_max)
            + u32::from(travel) / 2)
            / u32::from(travel)) as u16;
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

    fn resize_history(&mut self, row: u16) {
        let Some(bounds) = self.regions.history_bounds else {
            return;
        };
        let top = row.clamp(bounds.y, bounds.bottom().saturating_sub(3));
        self.settings.history_height = bounds.bottom().saturating_sub(top).max(3);
    }

    pub fn poll_worker(&mut self) {
        if self.mode == Mode::Picker {
            self.picker.poll_index();
        }
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
            KeyCode::Char('e') if self.view == View::Changes => self.toggle_left_pane(),
            KeyCode::Char('c') if self.view == View::Changes => {
                self.set_left_pane(LeftPane::Worktree);
                self.mode = Mode::Commit;
            }
            KeyCode::Char('a')
                if self.view == View::Changes && self.left_pane == LeftPane::Worktree =>
            {
                self.stage_all();
            }
            KeyCode::Char('u')
                if self.view == View::Changes && self.left_pane == LeftPane::Worktree =>
            {
                self.unstage_all();
            }
            KeyCode::Char(' ')
                if self.view == View::Changes
                    && self.left_pane == LeftPane::Worktree
                    && !self.history_focused =>
            {
                self.toggle_stage()
            }
            KeyCode::Enter if self.view == View::Changes && self.left_pane == LeftPane::Files => {
                self.toggle_selected_explorer_directory()
            }
            KeyCode::Enter
                if self.view == View::Changes
                    && self.left_pane == LeftPane::Worktree
                    && !self.history_focused =>
            {
                self.toggle_selected_directory()
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.view == View::Changes && self.left_pane == LeftPane::Files =>
            {
                self.expand_or_descend_explorer()
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.view == View::Changes
                    && self.left_pane == LeftPane::Worktree
                    && !self.history_focused =>
            {
                self.expand_or_descend_worktree()
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.view == View::Changes && self.left_pane == LeftPane::Files =>
            {
                self.collapse_or_ascend_explorer()
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.view == View::Changes
                    && self.left_pane == LeftPane::Worktree
                    && !self.history_focused =>
            {
                self.collapse_or_ascend_worktree()
            }
            KeyCode::PageDown if self.view == View::Changes => self.scroll_diff_by(10),
            KeyCode::PageUp if self.view == View::Changes => self.scroll_diff_by(-10),
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
                KeyCode::Esc => {
                    self.picker.editing_path = false;
                    self.picker.matches.clear();
                }
                KeyCode::Enter => self.confirm_picker_path(),
                KeyCode::Tab => self.picker.accept_completion(),
                KeyCode::Down => self.picker.move_match_selection(1),
                KeyCode::Up => self.picker.move_match_selection(-1),
                KeyCode::Backspace => {
                    self.picker.path_input.pop();
                    self.picker.refresh_matches();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.picker.path_input.clear();
                    self.picker.refresh_matches();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.picker.path_input.push(character);
                    self.picker.refresh_matches();
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
            KeyCode::Char('p') => self.picker.begin_search(Some("")),
            KeyCode::Char('/') => self
                .picker
                .begin_search(Some(std::path::MAIN_SEPARATOR_STR)),
            KeyCode::Char('r') => self.picker.reload(),
            KeyCode::Char('q') if self.repo.is_none() => self.should_quit = true,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.picker.begin_search(Some(&character.to_string()));
            }
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

    fn confirm_picker_path(&mut self) {
        let path = self.picker.selected_match_path();
        if !path.is_dir() {
            self.picker.error = Some(format!("Directory not found: {}", path.display()));
            return;
        }
        if is_repository_directory(&path) {
            self.open_repository(path);
        } else {
            self.picker.navigate(path);
            self.picker.editing_path = false;
            self.picker.matches.clear();
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
                self.collapsed_explorer_directories.clear();
                self.changes_state = ListState::default();
                self.explorer_state = ListState::default();
                self.graph_state = TableState::default();
                self.history_state = ListState::default();
                self.history_focused = false;
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
                if self.left_pane == LeftPane::Files {
                    self.move_explorer_selection(delta);
                } else if self.history_focused {
                    let len = self.repo.as_ref().map_or(0, |repo| repo.history.len());
                    move_list(&mut self.history_state, len, delta);
                    self.refresh_diff();
                } else {
                    let len = self.worktree_rows().len();
                    move_list(&mut self.changes_state, len, delta);
                    self.refresh_diff();
                }
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
                if self.left_pane == LeftPane::Files {
                    self.explorer_state
                        .select((!self.explorer_rows().is_empty()).then_some(0));
                    self.refresh_diff();
                } else if self.history_focused {
                    self.history_state.select(
                        self.repo
                            .as_ref()
                            .is_some_and(|repo| !repo.history.is_empty())
                            .then_some(0),
                    );
                    self.refresh_diff();
                } else {
                    self.changes_state.select(self.first_change_row());
                    self.refresh_diff();
                }
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
                if self.left_pane == LeftPane::Files {
                    self.explorer_state
                        .select(self.explorer_rows().len().checked_sub(1));
                    self.refresh_diff();
                } else if self.history_focused {
                    self.history_state.select(
                        self.repo
                            .as_ref()
                            .and_then(|repo| repo.history.len().checked_sub(1)),
                    );
                    self.refresh_diff();
                } else {
                    self.changes_state.select(self.last_change_row());
                    self.refresh_diff();
                }
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
        let selected_explorer_file = self.selected_explorer_file_path().map(str::to_owned);
        let selected_explorer_directory = self.selected_explorer_directory_path();
        let selected_oid = self
            .graph_state
            .selected()
            .and_then(|index| self.repo.as_ref()?.commits.get(index))
            .map(|commit| commit.oid.clone());
        let selected_history_oid = self
            .history_state
            .selected()
            .and_then(|index| self.repo.as_ref()?.history.get(index))
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
                let history_index = selected_history_oid
                    .and_then(|oid| repo.history.iter().position(|commit| commit.oid == oid));
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
                self.history_state.select(history_index);
                let explorer_row = selected_explorer_file
                    .and_then(|path| {
                        let file_index = self
                            .repo
                            .as_ref()?
                            .files
                            .iter()
                            .position(|candidate| candidate == &path)?;
                        self.row_for_explorer_file(file_index)
                    })
                    .or_else(|| {
                        let directory = selected_explorer_directory.as_ref()?;
                        self.explorer_rows()
                            .iter()
                            .position(|row| row.directory_path.as_ref() == Some(directory))
                    })
                    .or_else(|| self.first_explorer_file_row());
                self.explorer_state.select(explorer_row);
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
        if self.left_pane == LeftPane::Files {
            let rows = build_file_tree(&repo.files, &self.collapsed_explorer_directories);
            let Some(row) = self
                .explorer_state
                .selected()
                .and_then(|index| rows.get(index))
            else {
                self.diff = "Select a file to preview".to_owned();
                return;
            };
            if let Some(index) = row.file_index {
                self.diff = git::file_content(&repo.root, &repo.files[index])
                    .unwrap_or_else(|error| error.to_string());
            } else if let Some(path) = &row.directory_path {
                let count = repo
                    .files
                    .iter()
                    .filter(|file| file.starts_with(&format!("{path}/")))
                    .count();
                self.diff = format!("{count} files in {path}/");
            }
            return;
        }
        if self.history_focused
            && let Some(commit) = self
                .history_state
                .selected()
                .and_then(|index| repo.history.get(index))
        {
            self.diff =
                git::commit_diff(&repo.root, &commit.oid).unwrap_or_else(|error| error.to_string());
            return;
        }
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
        self.history_state.select(None);
        self.explorer_state.select(self.first_explorer_file_row());
    }

    pub fn worktree_rows(&self) -> Vec<WorktreeRow> {
        self.repo.as_ref().map_or_else(Vec::new, |repo| {
            build_worktree(&repo.changes, &self.collapsed_directories)
        })
    }

    pub fn explorer_rows(&self) -> Vec<ExplorerRow> {
        self.repo.as_ref().map_or_else(Vec::new, |repo| {
            build_file_tree(&repo.files, &self.collapsed_explorer_directories)
        })
    }

    pub fn selected_explorer_file_path(&self) -> Option<&str> {
        let selected = self.explorer_state.selected()?;
        let file_index = self.explorer_rows().get(selected)?.file_index?;
        self.repo
            .as_ref()?
            .files
            .get(file_index)
            .map(String::as_str)
    }

    fn selected_explorer_directory_path(&self) -> Option<String> {
        let selected = self.explorer_state.selected()?;
        self.explorer_rows().get(selected)?.directory_path.clone()
    }

    fn set_left_pane(&mut self, pane: LeftPane) {
        if self.left_pane == pane {
            return;
        }
        self.left_pane = pane;
        self.mode = Mode::Normal;
        self.clear_history_selection();
        if pane == LeftPane::Files && self.explorer_state.selected().is_none() {
            self.explorer_state.select(self.first_explorer_file_row());
        }
        self.refresh_diff();
    }

    fn toggle_left_pane(&mut self) {
        self.set_left_pane(match self.left_pane {
            LeftPane::Worktree => LeftPane::Files,
            LeftPane::Files => LeftPane::Worktree,
        });
    }

    fn move_explorer_selection(&mut self, delta: isize) {
        let len = self.explorer_rows().len();
        move_list(&mut self.explorer_state, len, delta);
        self.refresh_diff();
    }

    fn toggle_selected_explorer_directory(&mut self) {
        let Some(path) = self.selected_explorer_directory_path() else {
            return;
        };
        if !self.collapsed_explorer_directories.remove(&path) {
            self.collapsed_explorer_directories.insert(path.clone());
        }
        self.select_explorer_directory(&path);
        self.refresh_diff();
    }

    fn expand_or_descend_explorer(&mut self) {
        let rows = self.explorer_rows();
        let Some(index) = self.explorer_state.selected() else {
            return;
        };
        let Some(row) = rows.get(index) else { return };
        let Some(path) = &row.directory_path else {
            return;
        };
        if row.directory_expanded == Some(false) {
            self.collapsed_explorer_directories.remove(path);
            self.select_explorer_directory(path);
        } else if rows
            .get(index + 1)
            .is_some_and(|child| child.depth > row.depth)
        {
            self.explorer_state.select(Some(index + 1));
        }
        self.refresh_diff();
    }

    fn collapse_or_ascend_explorer(&mut self) {
        let rows = self.explorer_rows();
        let Some(index) = self.explorer_state.selected() else {
            return;
        };
        let Some(row) = rows.get(index) else { return };
        if let Some(path) = &row.directory_path
            && row.directory_expanded == Some(true)
        {
            self.collapsed_explorer_directories.insert(path.clone());
            self.select_explorer_directory(path);
            self.refresh_diff();
            return;
        }
        if let Some(parent) = rows[..index]
            .iter()
            .rposition(|candidate| candidate.depth < row.depth)
        {
            self.explorer_state.select(Some(parent));
            self.refresh_diff();
        }
    }

    fn select_explorer_directory(&mut self, path: &str) {
        let row = self
            .explorer_rows()
            .iter()
            .position(|row| row.directory_path.as_deref() == Some(path));
        self.explorer_state.select(row);
    }

    fn row_for_explorer_file(&self, file_index: usize) -> Option<usize> {
        self.explorer_rows()
            .iter()
            .position(|row| row.file_index == Some(file_index))
    }

    fn first_explorer_file_row(&self) -> Option<usize> {
        self.explorer_rows()
            .iter()
            .position(|row| row.file_index.is_some())
    }

    fn selected_change_index(&self) -> Option<usize> {
        let selected = self.changes_state.selected()?;
        self.worktree_rows().get(selected)?.change_index
    }

    fn selected_directory_path(&self) -> Option<String> {
        let selected = self.changes_state.selected()?;
        self.worktree_rows().get(selected)?.directory_path.clone()
    }

    fn clear_history_selection(&mut self) {
        self.history_focused = false;
        self.history_state.select(None);
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
            matches: Vec::new(),
            match_state: ListState::default(),
            searching: false,
            error: None,
            directory_index: Vec::new(),
            index_rx: None,
        };
        picker.reload();
        picker
    }

    fn reload(&mut self) {
        self.error = None;
        let current_is_repo = is_repository_directory(&self.directory);
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

    fn move_match_selection(&mut self, delta: isize) {
        move_list(&mut self.match_state, self.matches.len(), delta);
    }

    fn begin_search(&mut self, initial: Option<&str>) {
        self.editing_path = true;
        self.error = None;
        if let Some(initial) = initial {
            self.path_input = initial.to_owned();
        }
        self.refresh_matches();
    }

    fn poll_index(&mut self) {
        let Some(receiver) = &self.index_rx else {
            return;
        };
        let Ok(index) = receiver.try_recv() else {
            return;
        };
        self.directory_index = index;
        self.index_rx = None;
        self.searching = false;
        self.refresh_matches();
    }

    fn refresh_matches(&mut self) {
        self.error = None;
        let query = self.path_input.trim();
        if query.is_empty() {
            self.matches.clear();
            self.match_state.select(None);
            return;
        }
        if !query.contains(['/', '\\'])
            && self.directory_index.is_empty()
            && self.index_rx.is_none()
        {
            self.searching = true;
            let (sender, receiver) = mpsc::channel();
            self.index_rx = Some(receiver);
            let roots = search_roots(&self.directory);
            thread::spawn(move || {
                let _ = sender.send(index_directories(&roots));
            });
        }

        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        if let Some(path) = resolve_fuzzy_path(query, &self.directory)
            && seen.insert(path.clone())
        {
            candidates.push((u32::MAX, path));
        }
        for path in &self.directory_index {
            let Some(score) = fuzzy_path_score(query, path) else {
                continue;
            };
            if seen.insert(path.clone()) {
                candidates.push((score, path.clone()));
            }
        }
        candidates.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| path_depth(left).cmp(&path_depth(right)))
                .then_with(|| left.cmp(right))
        });
        self.matches = candidates
            .into_iter()
            .take(12)
            .map(|(_, path)| PickerEntry {
                label: display_search_path(&path),
                is_repo: is_repository_directory(&path),
                path,
                action: PickerAction::Navigate,
            })
            .collect();
        self.match_state
            .select((!self.matches.is_empty()).then_some(0));
    }

    fn accept_completion(&mut self) {
        let Some(path) = self
            .match_state
            .selected()
            .and_then(|index| self.matches.get(index))
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        self.path_input = path.display().to_string();
        self.refresh_matches();
    }

    fn selected_match_path(&self) -> PathBuf {
        self.match_state
            .selected()
            .and_then(|index| self.matches.get(index))
            .map(|entry| entry.path.clone())
            .unwrap_or_else(|| self.input_path())
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
        let expanded = expand_search_path(self.path_input.trim());
        if expanded.is_absolute() {
            expanded
        } else {
            self.directory.join(expanded)
        }
    }
}

fn search_roots(current: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home_directory() {
        roots.push(home);
    }
    if !roots.iter().any(|root| current.starts_with(root)) {
        roots.push(current.to_path_buf());
    }
    for path in ["/workspace", "/workspaces", "/projects", "/mnt", "/media"] {
        let path = PathBuf::from(path);
        if path.is_dir() {
            roots.push(path);
        }
    }
    roots
}

fn index_directories(roots: &[PathBuf]) -> Vec<PathBuf> {
    const MAX_DIRECTORIES: usize = 25_000;
    const MAX_DEPTH: usize = 7;
    let mut directories = Vec::new();
    let mut queue: VecDeque<_> = roots.iter().cloned().map(|path| (path, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((directory, depth)) = queue.pop_front() {
        if directories.len() >= MAX_DIRECTORIES || !seen.insert(directory.clone()) {
            continue;
        }
        directories.push(directory.clone());
        if depth >= MAX_DEPTH || is_bare_repository_directory(&directory) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if should_skip_index_directory(&name) {
                continue;
            }
            queue.push_back((entry.path(), depth + 1));
        }
    }
    directories
}

fn should_skip_index_directory(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "vendor" | "dist" | "build" | "__pycache__"
        )
}

fn expand_search_path(input: &str) -> PathBuf {
    if input == "~" {
        home_directory().unwrap_or_else(|| PathBuf::from(input))
    } else if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        home_directory()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(input))
    } else {
        PathBuf::from(input)
    }
}

fn resolve_fuzzy_path(input: &str, base: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let expanded = expand_search_path(input);
    let mut resolved = if expanded.is_absolute() {
        PathBuf::new()
    } else {
        base.to_path_buf()
    };
    for component in expanded.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let exact = resolved.join(name);
                if exact.is_dir() {
                    resolved = exact;
                    continue;
                }
                let query = name.to_string_lossy();
                let entries = fs::read_dir(&resolved).ok()?;
                let best = entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter_map(|entry| {
                        let score = fuzzy_text_score(&query, &entry.file_name().to_string_lossy())?;
                        Some((score, entry.path()))
                    })
                    .max_by(|(left_score, left), (right_score, right)| {
                        left_score.cmp(right_score).then_with(|| right.cmp(left))
                    })?;
                resolved = best.1;
            }
        }
    }
    resolved.is_dir().then_some(resolved)
}

fn fuzzy_path_score(query: &str, path: &Path) -> Option<u32> {
    let query = query.trim_matches(['/', '\\']);
    if query.is_empty() {
        return None;
    }
    let name = path.file_name()?.to_string_lossy();
    fuzzy_text_score(query, &name).map(|score| {
        score
            + if is_repository_directory(path) {
                750
            } else {
                0
            }
    })
}

fn fuzzy_text_score(query: &str, candidate: &str) -> Option<u32> {
    let query = query.to_lowercase();
    let candidate = candidate.to_lowercase();
    let query_len = query.chars().count();
    if query == candidate {
        return Some(10_000);
    }
    if candidate.starts_with(&query) {
        return Some(9_000u32.saturating_sub(candidate.len() as u32));
    }
    if let Some(index) = candidate.find(&query) {
        return Some(8_000u32.saturating_sub(index as u32));
    }
    let mut positions = Vec::new();
    let mut offset = 0;
    for needle in query.chars() {
        let relative = candidate[offset..].find(needle)?;
        offset += relative;
        positions.push(offset);
        offset += needle.len_utf8();
    }
    let span = positions.last()? - positions.first()?;
    if span > query_len.saturating_mul(3).max(4) {
        return None;
    }
    Some(6_000u32.saturating_sub(span as u32))
}

fn is_repository_directory(path: &Path) -> bool {
    path.join(".git").exists()
}

fn is_bare_repository_directory(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
}

fn display_search_path(path: &Path) -> String {
    if let Some(home) = home_directory()
        && let Ok(relative) = path.strip_prefix(home)
    {
        return if relative.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", relative.display())
        };
    }
    path.display().to_string()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
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
            "history_height" => {
                if let Ok(height) = value.trim().parse::<u16>() {
                    settings.history_height = height.clamp(3, 256);
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
            "auto_fetch={}\nfetch_interval_minutes={}\nworktree_width={}\nhistory_height={}\n",
            settings.auto_fetch,
            settings.fetch_interval_minutes,
            settings.worktree_width,
            settings.history_height
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
    fn builds_a_collapsible_repository_file_tree() {
        let files = vec![
            "src/app/mod.rs".to_owned(),
            "src/app/view.rs".to_owned(),
            "src/main.rs".to_owned(),
            "README.md".to_owned(),
        ];
        let rows = build_file_tree(&files, &HashSet::new());
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            ["src", "app", "mod.rs", "view.rs", "main.rs", "README.md"]
        );
        assert_eq!(rows[2].file_index, Some(0));
        assert_eq!(rows[4].file_index, Some(2));

        let rows = build_file_tree(&files, &HashSet::from(["src/app".to_owned()]));
        assert!(rows.iter().any(|row| {
            row.directory_path.as_deref() == Some("src/app")
                && row.directory_expanded == Some(false)
        }));
        assert!(!rows.iter().any(|row| row.label == "mod.rs"));
    }

    #[test]
    fn fuzzy_repository_paths_resolve_and_complete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let code = root.join("code");
        let gitui = code.join("gitui");
        let gitlab = code.join("gitlab-runner");
        fs::create_dir_all(gitui.join(".git")).unwrap();
        fs::create_dir_all(&gitlab).unwrap();

        assert_eq!(resolve_fuzzy_path("cod/gitu", root), Some(gitui.clone()));

        let mut picker = RepositoryPicker::new(root.to_path_buf());
        picker.directory_index = vec![gitlab, gitui.clone()];
        picker.begin_search(Some("gtu"));
        assert_eq!(picker.matches[0].path, gitui);
        assert!(picker.matches[0].is_repo);
        assert!(fuzzy_text_score("gitui", "go-genai-streamed-function-args").is_none());

        picker.accept_completion();
        assert_eq!(PathBuf::from(&picker.path_input), picker.matches[0].path);
    }

    #[test]
    fn directory_index_skips_build_trees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("projects/gitui")).unwrap();
        fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        fs::create_dir_all(root.join("archive.git/objects/pack")).unwrap();
        fs::create_dir_all(root.join("archive.git/refs")).unwrap();
        fs::write(root.join("archive.git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let index = index_directories(&[root.to_path_buf()]);
        assert!(index.contains(&root.join("projects/gitui")));
        assert!(!index.contains(&root.join("target")));
        assert!(index.contains(&root.join("archive.git")));
        assert!(!index.contains(&root.join("archive.git/objects")));
    }

    #[test]
    fn persists_auto_fetch_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config");
        let settings = Settings {
            auto_fetch: true,
            fetch_interval_minutes: 17,
            worktree_width: 61,
            history_height: 9,
        };

        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);

        fs::write(
            &path,
            "auto_fetch=true\nfetch_interval_minutes=0\nworktree_width=5\nhistory_height=1\n",
        )
        .unwrap();
        let loaded = load_settings(&path);
        assert_eq!(loaded.fetch_interval_minutes, 1);
        assert_eq!(loaded.worktree_width, 24);
        assert_eq!(loaded.history_height, 3);
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
                history_height: 7,
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
