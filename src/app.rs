mod actions;
mod changes;
mod repository_picker;

pub(crate) use actions::{ACTION_ITEMS, ActionsState, CommandStatus};
pub use changes::{ChangesState, LeftPane};
pub use repository_picker::{PickerAction, PickerEntry, RepositoryPicker};

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Position, Rect},
    widgets::TableState,
};

use crate::{
    git::{self, RepositoryData},
    repository_session::{RepositorySession, WorkerCompletion},
};

use actions::{ActionId, action_command, display_git_command, parse_git_args};
use repository_picker::PickerCommand;

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
    ActionMenu,
    Command,
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

#[derive(Debug, Default, Clone, Copy)]
pub struct Regions {
    pub changes: Option<Rect>,
    pub graph: Option<Rect>,
    pub refresh: Option<Rect>,
    pub repository: Option<Rect>,
    pub settings: Option<Rect>,
    pub help: Option<Rect>,
    pub actions: Option<Rect>,
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
    pub action_menu: Option<Rect>,
    pub action_list: Option<Rect>,
    pub command_overlay: Option<Rect>,
    pub command_output: Option<Rect>,
    pub auto_fetch: Option<Rect>,
    pub fetch_interval: Option<Rect>,
    pub fetch_interval_down: Option<Rect>,
    pub fetch_interval_up: Option<Rect>,
    pub stage_all: Option<Rect>,
    pub unstage_all: Option<Rect>,
}

pub struct App {
    pub(crate) session: RepositorySession,
    pub view: View,
    pub mode: Mode,
    pub changes: ChangesState,
    pub graph_state: TableState,
    pub commit_message: String,
    pub dragging_splitter: bool,
    pub dragging_history: bool,
    pub dragging_diff_scrollbar: bool,
    diff_scroll_drag_offset: u16,
    pub picker: RepositoryPicker,
    pub(crate) actions: ActionsState,
    pub settings: Settings,
    pub settings_selection: usize,
    pub notice: Option<String>,
    pub regions: Regions,
    pub should_quit: bool,
    pub(crate) settings_path: Option<PathBuf>,
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let settings_path = settings_path();
        let settings = settings_path
            .as_deref()
            .map(load_settings)
            .unwrap_or_default();
        let session = RepositorySession::new(&path, fetch_interval(&settings));
        let mode = if session.data().is_some() {
            Mode::Normal
        } else {
            Mode::Picker
        };
        let start = session
            .data()
            .and_then(|repo| repo.root.parent().map(Path::to_path_buf))
            .unwrap_or(path);

        let changes = ChangesState::new(session.data());
        let mut graph_state = TableState::default();
        graph_state.select(
            session
                .data()
                .is_some_and(|repo| !repo.commits.is_empty())
                .then_some(0),
        );
        Self {
            session,
            view: View::Changes,
            mode,
            changes,
            graph_state,
            commit_message: String::new(),
            dragging_splitter: false,
            dragging_history: false,
            dragging_diff_scrollbar: false,
            diff_scroll_drag_offset: 0,
            picker: RepositoryPicker::new(start),
            actions: ActionsState::default(),
            settings,
            settings_selection: 0,
            notice: None,
            regions: Regions::default(),
            should_quit: false,
            settings_path,
        }
    }

    pub(crate) fn repository(&self) -> Option<&RepositoryData> {
        self.session.data()
    }

    pub(crate) fn commit_running(&self) -> bool {
        self.session.commit_running()
    }

    pub(crate) fn fetch_running(&self) -> bool {
        self.session.fetch_running()
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
            Mode::ActionMenu => self.handle_action_menu(key),
            Mode::Command => self.handle_command(key),
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
            Mode::Picker => self.picker.paste(text),
            Mode::Command if self.actions.status != CommandStatus::Running => {
                self.actions.input.push_str(text);
                if self.actions.status == CommandStatus::Input {
                    self.actions.stderr.clear();
                }
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.mode == Mode::ActionMenu {
            self.handle_action_mouse(mouse);
            return;
        }
        if self.mode == Mode::Command {
            self.handle_command_mouse(mouse);
            return;
        }
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
            .actions
            .is_some_and(|rect| rect.contains(point))
        {
            self.open_actions();
            return;
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
            self.changes.history_focused = true;
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
            self.changes.clear_history_selection();
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
            if self.changes.selected_explorer_directory_path().is_some() {
                let repo = self.session.data();
                self.changes.toggle_selected_explorer_directory(repo);
            }
        } else if self.select_worktree_row(point) {
            if self
                .repository()
                .and_then(|repo| self.changes.selected_directory_path(repo))
                .is_some()
            {
                let repo = self.session.data();
                self.changes.toggle_selected_directory(repo);
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
            self.changes.clear_history_selection();
            self.changes.refresh_diff(self.session.data());
        } else if self.select_history_row(point) {
        } else if self.select_graph_row(point) {
            self.view = View::Graph;
        } else if self.regions.commit.is_some_and(|rect| rect.contains(point)) {
            self.mode = Mode::Commit;
        }
    }

    fn handle_action_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.actions.move_selection(1),
            MouseEventKind::ScrollUp => self.actions.move_selection(-1),
            MouseEventKind::Moved => {
                if let Some(index) = self.action_at(point) {
                    self.actions.selection = index;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .regions
                    .actions
                    .is_some_and(|rect| rect.contains(point))
                {
                    self.mode = Mode::Normal;
                    return;
                }
                let Some(index) = self.action_at(point) else {
                    self.mode = Mode::Normal;
                    return;
                };
                self.actions.selection = index;
                self.activate_action();
            }
            _ => {}
        }
    }

    fn action_at(&self, point: Position) -> Option<usize> {
        let list = self
            .regions
            .action_list
            .filter(|rect| rect.contains(point))?;
        let index = usize::from(point.y.saturating_sub(list.y));
        (index < ACTION_ITEMS.len()).then_some(index)
    }

    fn handle_command_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollDown => self.actions.scroll_by(3),
            MouseEventKind::ScrollUp => self.actions.scroll_by(-3),
            _ => {}
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
                    && self.repository().is_some()
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
                        let command = self.picker.confirm_path();
                        self.apply_picker_command(command);
                    }
                } else if index < self.picker.entries.len() {
                    self.picker.state.select(Some(index));
                    let command = self.picker.activate_selected(true);
                    self.apply_picker_command(command);
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
        if self.view != View::Changes || self.changes.pane != LeftPane::Worktree {
            return false;
        }
        let Some(rect) = self
            .regions
            .worktree_list
            .filter(|rect| rect.contains(point))
        else {
            return false;
        };
        let index = self.changes.worktree_scroll + usize::from(point.y - rect.y);
        let Some(repo) = self.session.data() else {
            return false;
        };
        self.changes.select_worktree_row(repo, index)
    }

    fn select_explorer_row(&mut self, point: Position) -> bool {
        if self.view != View::Changes || self.changes.pane != LeftPane::Files {
            return false;
        }
        let Some(rect) = self
            .regions
            .explorer_list
            .filter(|rect| rect.contains(point))
        else {
            return false;
        };
        let index = self.changes.explorer_scroll + usize::from(point.y - rect.y);
        let Some(repo) = self.session.data() else {
            return false;
        };
        self.changes.select_explorer_row(repo, index)
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
        let Some(repo) = self.session.data() else {
            return false;
        };
        let relative_row = usize::from(point.y - rect.y);
        self.changes.select_history_row(repo, relative_row)
    }

    fn select_graph_row(&mut self, point: Position) -> bool {
        if self.view != View::Graph {
            return false;
        }
        let Some(rect) = self.regions.graph_table.filter(|rect| rect.contains(point)) else {
            return false;
        };
        let index = self.graph_state.offset() + usize::from(point.y - rect.y);
        let len = self.repository().map_or(0, |repo| repo.commits.len());
        if index >= len {
            return false;
        }
        self.graph_state.select(Some(index));
        true
    }

    fn scroll_at(&mut self, point: Position, delta: isize) {
        if self.regions.diff.is_some_and(|rect| rect.contains(point)) {
            self.changes
                .scroll_diff_by(self.regions.diff_scroll_max, delta.saturating_mul(3));
        } else if self
            .regions
            .explorer_list
            .is_some_and(|rect| rect.contains(point))
        {
            self.scroll_explorer(delta.saturating_mul(3));
        } else if self
            .regions
            .history_list
            .is_some_and(|rect| rect.contains(point))
        {
            if let Some(repo) = self.session.data() {
                self.changes.move_history_selection(repo, delta);
            }
        } else if self
            .regions
            .worktree_list
            .is_some_and(|rect| rect.contains(point))
        {
            self.scroll_worktree(delta.saturating_mul(3));
        } else if self
            .regions
            .graph_table
            .is_some_and(|rect| rect.contains(point))
        {
            self.move_selection(delta);
        }
    }

    fn scroll_worktree(&mut self, delta: isize) {
        let viewport = self
            .regions
            .worktree_list
            .map_or(0, |rect| usize::from(rect.height));
        self.changes
            .scroll_worktree(self.session.data(), viewport, delta);
    }

    fn scroll_explorer(&mut self, delta: isize) {
        let viewport = self
            .regions
            .explorer_list
            .map_or(0, |rect| usize::from(rect.height));
        self.changes.scroll_explorer(viewport, delta);
    }

    fn scroll_diff_by(&mut self, delta: isize) {
        self.changes
            .scroll_diff_by(self.regions.diff_scroll_max, delta);
    }

    fn scroll_diff_to(&mut self, row: u16) {
        let Some(track) = self.regions.diff_scrollbar else {
            return;
        };
        let Some(thumb) = self.regions.diff_scroll_thumb else {
            return;
        };
        self.changes.set_diff_scroll_from_track(
            row,
            track.y,
            track.height,
            thumb.height,
            self.diff_scroll_drag_offset,
            self.regions.diff_scroll_max,
        );
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
        let interval = fetch_interval(&self.settings);
        self.session
            .maybe_start_fetch(self.settings.auto_fetch, interval);
        self.session.maybe_start_status_check();
        while let Some(done) = self.session.next_worker_completion(interval) {
            match done {
                WorkerCompletion::Commit(result) => match result {
                    Ok(output) if output.success => {
                        self.commit_message.clear();
                        self.reload();
                        self.notice = Some("Commit created".to_owned());
                    }
                    Ok(output) => {
                        self.notice = Some(first_error(&output.stderr, "Commit failed"));
                    }
                    Err(error) => self.notice = Some(error),
                },
                WorkerCompletion::Fetch(result) => match result {
                    Ok(output) if output.success => {
                        self.reload();
                        self.notice = Some("Fetched remotes".to_owned());
                    }
                    Ok(output) => {
                        self.notice = Some(first_error(&output.stderr, "Fetch failed"));
                    }
                    Err(error) => self.notice = Some(error),
                },
                WorkerCompletion::Command(done) => match done.result {
                    Ok(output) => {
                        let success = output.success;
                        let error = first_error(&output.stderr, "Git command failed");
                        if success {
                            self.reload();
                        }
                        self.actions.complete(output);
                        self.notice = Some(if success {
                            format!("{} complete", done.label)
                        } else {
                            error
                        });
                    }
                    Err(error) => {
                        self.actions.fail(error.clone());
                        self.notice = Some(error);
                    }
                },
            }
        }
        while self.session.next_worktree_change() {
            self.reload();
            if self.notice.as_deref() == Some("Refreshed") {
                self.notice = None;
            }
        }
    }

    pub fn change_counts(&self) -> (usize, usize) {
        self.repository().map_or((0, 0), |repo| {
            (
                repo.changes.iter().filter(|change| change.staged).count(),
                repo.changes.iter().filter(|change| !change.staged).count(),
            )
        })
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') if self.commit_running() || self.session.command_running() => {
                self.notice = Some("A Git operation is still running".to_owned())
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
            KeyCode::Char('x') => self.open_actions(),
            KeyCode::Char('g') => self.open_git_command(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('w') if self.view == View::Changes => {
                let wrapped = self.changes.toggle_wrap();
                self.notice = Some(
                    if wrapped {
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
                if self.view == View::Changes && self.changes.pane == LeftPane::Worktree =>
            {
                self.stage_all();
            }
            KeyCode::Char('u')
                if self.view == View::Changes && self.changes.pane == LeftPane::Worktree =>
            {
                self.unstage_all();
            }
            KeyCode::Char(' ')
                if self.view == View::Changes
                    && self.changes.pane == LeftPane::Worktree
                    && !self.changes.history_focused =>
            {
                self.toggle_stage()
            }
            KeyCode::Enter
                if self.view == View::Changes && self.changes.pane == LeftPane::Files =>
            {
                let repo = self.session.data();
                self.changes.toggle_selected_explorer_directory(repo);
            }
            KeyCode::Enter
                if self.view == View::Changes
                    && self.changes.pane == LeftPane::Worktree
                    && !self.changes.history_focused =>
            {
                let repo = self.session.data();
                self.changes.toggle_selected_directory(repo);
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.view == View::Changes && self.changes.pane == LeftPane::Files =>
            {
                let repo = self.session.data();
                self.changes.expand_or_descend_explorer(repo);
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.view == View::Changes
                    && self.changes.pane == LeftPane::Worktree
                    && !self.changes.history_focused =>
            {
                let repo = self.session.data();
                self.changes.expand_or_descend_worktree(repo);
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.view == View::Changes && self.changes.pane == LeftPane::Files =>
            {
                let repo = self.session.data();
                self.changes.collapse_or_ascend_explorer(repo);
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.view == View::Changes
                    && self.changes.pane == LeftPane::Worktree
                    && !self.changes.history_focused =>
            {
                let repo = self.session.data();
                self.changes.collapse_or_ascend_worktree(repo);
            }
            KeyCode::PageDown if self.view == View::Changes => self.scroll_diff_by(10),
            KeyCode::PageUp if self.view == View::Changes => self.scroll_diff_by(-10),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Home => self.select_first(),
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
        let command = self.picker.handle_key(key, self.repository().is_some());
        self.apply_picker_command(command);
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

    fn handle_action_menu(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('x') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.actions.move_selection(1);
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.actions.move_selection(-1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_action(),
            _ => {}
        }
    }

    fn handle_command(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            return;
        }
        if self.actions.status != CommandStatus::Running {
            match key.code {
                KeyCode::Enter => {
                    let input = if self.actions.input.trim().is_empty()
                        && matches!(self.actions.status, CommandStatus::Complete { .. })
                    {
                        self.actions.command.clone()
                    } else {
                        self.actions.input.clone()
                    };
                    match parse_git_args(&input) {
                        Ok(args) => self.start_git_command("Git command".to_owned(), args),
                        Err(error) => {
                            self.actions.status = CommandStatus::Input;
                            self.actions.stderr = error;
                        }
                    }
                }
                KeyCode::Down if matches!(self.actions.status, CommandStatus::Complete { .. }) => {
                    self.actions.scroll_by(1);
                }
                KeyCode::Up if matches!(self.actions.status, CommandStatus::Complete { .. }) => {
                    self.actions.scroll_by(-1);
                }
                KeyCode::PageDown
                    if matches!(self.actions.status, CommandStatus::Complete { .. }) =>
                {
                    self.actions.scroll_by(10);
                }
                KeyCode::PageUp
                    if matches!(self.actions.status, CommandStatus::Complete { .. }) =>
                {
                    self.actions.scroll_by(-10);
                }
                KeyCode::Backspace => {
                    self.actions.input.pop();
                    if self.actions.status == CommandStatus::Input {
                        self.actions.stderr.clear();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.actions.input.clear();
                    if self.actions.status == CommandStatus::Input {
                        self.actions.stderr.clear();
                    }
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.actions.input.push(character);
                    if self.actions.status == CommandStatus::Input {
                        self.actions.stderr.clear();
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.actions.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.actions.scroll_by(-1),
            KeyCode::PageDown => self.actions.scroll_by(10),
            KeyCode::PageUp => self.actions.scroll_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.actions.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.actions.scroll = self.actions.scroll_max,
            _ => {}
        }
    }

    fn open_actions(&mut self) {
        if self.repository().is_none() {
            self.notice = Some("Open a repository first".to_owned());
            return;
        }
        self.mode = Mode::ActionMenu;
    }

    fn open_git_command(&mut self) {
        if self.repository().is_none() {
            self.notice = Some("Open a repository first".to_owned());
            return;
        }
        self.actions.begin_input();
        self.mode = Mode::Command;
    }

    fn activate_action(&mut self) {
        let action = self.actions.selected();
        if action == ActionId::Custom {
            self.open_git_command();
            return;
        }
        if let Some((label, args)) = action_command(action) {
            self.start_git_command(label.to_owned(), args);
        }
    }

    fn start_git_command(&mut self, label: String, args: Vec<String>) {
        let display = display_git_command(&args);
        if self.session.start_command(label, args) {
            self.actions.begin_command(display);
            self.mode = Mode::Command;
            self.notice = None;
        } else {
            self.mode = Mode::Normal;
            self.notice = Some("Another Git operation is already running".to_owned());
        }
    }

    fn apply_picker_command(&mut self, command: PickerCommand) {
        match command {
            PickerCommand::None => {}
            PickerCommand::Close => self.mode = Mode::Normal,
            PickerCommand::Quit => self.should_quit = true,
            PickerCommand::Open(path) => self.open_repository(path),
        }
    }

    fn open_repository(&mut self, path: PathBuf) {
        match self.session.open(&path, fetch_interval(&self.settings)) {
            Ok(()) => {
                self.mode = Mode::Normal;
                self.actions = ActionsState::default();
                self.notice = Some("Repository opened".to_owned());
                self.graph_state = TableState::default();
                self.changes.reset_repository(self.session.data());
                self.graph_state.select(
                    self.session
                        .data()
                        .is_some_and(|repo| !repo.commits.is_empty())
                        .then_some(0),
                );
            }
            Err(error) => self.picker.error = Some(error.to_string()),
        }
    }

    fn open_picker(&mut self) {
        let start = self
            .repository()
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
        self.session
            .reset_fetch_deadline(fetch_interval(&self.settings));
        self.persist_settings();
    }

    fn persist_settings(&mut self) {
        if let Some(path) = &self.settings_path
            && let Err(error) = save_settings(path, &self.settings)
        {
            self.notice = Some(format!("Could not save settings: {error}"));
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.view {
            View::Changes => {
                let worktree_viewport = self
                    .regions
                    .worktree_list
                    .map_or(0, |rect| usize::from(rect.height));
                let explorer_viewport = self
                    .regions
                    .explorer_list
                    .map_or(0, |rect| usize::from(rect.height));
                self.changes.move_selection(
                    self.session.data(),
                    delta,
                    worktree_viewport,
                    explorer_viewport,
                );
            }
            View::Graph => {
                let len = self.repository().map_or(0, |repo| repo.commits.len());
                move_table(&mut self.graph_state, len, delta);
            }
        }
    }

    fn select_first(&mut self) {
        match self.view {
            View::Changes => {
                let worktree_viewport = self
                    .regions
                    .worktree_list
                    .map_or(0, |rect| usize::from(rect.height));
                let explorer_viewport = self
                    .regions
                    .explorer_list
                    .map_or(0, |rect| usize::from(rect.height));
                self.changes.select_first(
                    self.session.data(),
                    worktree_viewport,
                    explorer_viewport,
                );
            }
            View::Graph => self.graph_state.select(
                self.repository()
                    .is_some_and(|repo| !repo.commits.is_empty())
                    .then_some(0),
            ),
        }
    }

    fn select_last(&mut self) {
        match self.view {
            View::Changes => {
                let worktree_viewport = self
                    .regions
                    .worktree_list
                    .map_or(0, |rect| usize::from(rect.height));
                let explorer_viewport = self
                    .regions
                    .explorer_list
                    .map_or(0, |rect| usize::from(rect.height));
                self.changes
                    .select_last(self.session.data(), worktree_viewport, explorer_viewport);
            }
            View::Graph => self.graph_state.select(
                self.repository()
                    .and_then(|repo| repo.commits.len().checked_sub(1)),
            ),
        }
    }

    fn toggle_stage(&mut self) {
        let Some(repo) = self.repository() else {
            return;
        };
        let Some(index) = self.changes.selected_change_index(repo) else {
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
        let Some(root) = self.repository().map(|repo| repo.root.clone()) else {
            return;
        };
        if let Err(error) = git::stage_all(&root) {
            self.notice = Some(error.to_string());
        } else {
            self.reload();
        }
    }

    fn toggle_all_staging(&mut self) {
        let all_staged = self.repository().is_some_and(|repo| {
            !repo.changes.is_empty() && repo.changes.iter().all(|change| change.staged)
        });
        if all_staged {
            self.unstage_all();
        } else {
            self.stage_all();
        }
    }

    fn unstage_all(&mut self) {
        let Some(root) = self.repository().map(|repo| repo.root.clone()) else {
            return;
        };
        if let Err(error) = git::unstage_all(&root) {
            self.notice = Some(error.to_string());
        } else {
            self.reload();
        }
    }

    fn reload(&mut self) {
        let Some(repo) = self.repository() else {
            return;
        };
        let selection = self.changes.capture_selection(repo);
        let selected_oid = self
            .graph_state
            .selected()
            .and_then(|index| repo.commits.get(index))
            .map(|commit| commit.oid.clone());

        match self.session.reload() {
            Ok(()) => {
                let repo = self.session.data().expect("reloaded repository");
                let commit_index = selected_oid
                    .and_then(|oid| repo.commits.iter().position(|commit| commit.oid == oid));
                self.graph_state
                    .select(commit_index.or_else(|| repo.commits.first().map(|_| 0)));
                self.changes.restore_selection(repo, selection);
                self.notice = Some("Refreshed".to_owned());
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    pub fn selected_explorer_file_path(&self) -> Option<&str> {
        self.changes
            .selected_explorer_file_path(self.session.data()?)
    }

    fn set_left_pane(&mut self, pane: LeftPane) {
        if self.changes.set_pane(pane, self.session.data()) {
            self.mode = Mode::Normal;
        }
    }

    fn toggle_left_pane(&mut self) {
        self.set_left_pane(match self.changes.pane {
            LeftPane::Worktree => LeftPane::Files,
            LeftPane::Files => LeftPane::Worktree,
        });
    }

    fn start_commit(&mut self) {
        if self.session.commit_running() {
            self.notice = Some("A commit is already running".to_owned());
            return;
        }
        if self.session.command_running() {
            self.notice = Some("Another Git operation is already running".to_owned());
            return;
        }
        let message = self.commit_message.trim().to_owned();
        if message.is_empty() {
            self.notice = Some("Commit message cannot be empty".to_owned());
            return;
        }
        if self.session.start_commit(message) {
            self.mode = Mode::Normal;
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

fn move_table(state: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0);
    let next = (current as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
    state.select(Some(next));
}

#[cfg(test)]
mod tests {
    use std::{process::Command, thread};

    use super::*;

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
        app.changes.diff_scroll = 37;
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(app.changes.diff_wrap);
        assert_eq!(app.changes.diff_scroll, 37);
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(!app.changes.diff_wrap);
        assert_eq!(app.changes.diff_scroll, 37);
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
        app.session.schedule_fetch_now();
        app.poll_worker();
        assert!(app.fetch_running());

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.fetch_running() {
                break;
            }
        }
        assert!(!app.fetch_running());
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
        assert!(app.commit_running());
        assert_eq!(app.commit_message, "commit from control enter");

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.commit_running() {
                break;
            }
        }
        assert!(!app.commit_running());
        assert!(app.commit_message.is_empty());
        assert_eq!(app.repository().unwrap().commits.len(), 2);
    }

    #[test]
    fn runs_a_custom_git_command_and_keeps_its_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let mut app = App::new(root.to_path_buf());

        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.actions.status, CommandStatus::Input);

        app.handle_paste("rev-parse --abbrev-ref HEAD");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.actions.status, CommandStatus::Running);
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.session.command_running() {
                break;
            }
        }

        assert_eq!(
            app.actions.status,
            CommandStatus::Complete {
                success: true,
                exit_code: Some(0),
            }
        );
        assert_eq!(app.actions.stdout.trim(), "main");
        assert_eq!(app.actions.command, "git rev-parse --abbrev-ref HEAD");
        assert!(app.actions.input.is_empty());
        assert_eq!(app.actions.transcript.len(), 1);
        assert_eq!(app.actions.transcript[0].stdout.trim(), "main");

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        app.handle_paste("tatus --short");
        assert_eq!(app.actions.input, "status --short");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.actions.status, CommandStatus::Running);
        assert_eq!(app.actions.transcript.len(), 1);
        assert_eq!(app.actions.transcript[0].stdout.trim(), "main");
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            app.poll_worker();
            if !app.session.command_running() {
                break;
            }
        }
        assert_eq!(
            app.actions.status,
            CommandStatus::Complete {
                success: true,
                exit_code: Some(0),
            }
        );
        assert_eq!(app.actions.command, "git status --short");
        assert!(app.actions.input.is_empty());
        assert_eq!(app.actions.transcript.len(), 2);
        assert_eq!(
            app.actions.transcript[0].command,
            "git rev-parse --abbrev-ref HEAD"
        );
        assert_eq!(app.actions.transcript[0].stdout.trim(), "main");
        assert_eq!(app.actions.transcript[1].command, "git status --short");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn refreshes_an_already_dirty_file_when_its_contents_change_again() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let tracked = root.join("tracked.txt");
        let mut app = App::new(root.to_path_buf());

        fs::write(&tracked, "first\n").unwrap();
        app.session.schedule_status_check_now();
        for _ in 0..100 {
            app.poll_worker();
            if app.changes.diff.contains("first") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.changes.diff.contains("first"));

        fs::write(&tracked, "later\n").unwrap();
        app.session.schedule_status_check_now();
        for _ in 0..100 {
            app.poll_worker();
            if app.changes.diff.contains("later") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.changes.diff.contains("later"));
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
}
