mod actions;
mod changes;
mod file_search;
mod fuzzy;
mod repository_browser;
mod repository_picker;
mod text_input;

pub(crate) use actions::{ACTION_ITEMS, ActionsState, CommandStatus};
pub(crate) use changes::PreviewRenderCache;
pub use changes::{ChangesState, LeftPane};
pub(crate) use file_search::FileSearch;
pub(crate) use repository_browser::{BrowserTab, RemoteItems, RepositoryBrowser};
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
    filesystem::{FileOperation, validate_name},
    git::RepositoryData,
    repository_session::{LoadKind, Mutation, RepositorySession, WorkerCompletion},
    selection::{SelectionOutcome, SelectionState},
};

use actions::{ActionId, action_command, display_git_command, parse_command_args, parse_git_args};
use repository_picker::PickerCommand;
pub(crate) use text_input::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Changes,
    Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Commit,
    FileSearch,
    Picker,
    Settings,
    Help,
    RepositoryBrowser,
    ActionMenu,
    Command,
    Editor,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub auto_fetch: bool,
    pub fetch_interval_minutes: u16,
    pub worktree_width: u16,
    pub history_height: u16,
    pub editor_command: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_fetch: false,
            fetch_interval_minutes: 5,
            worktree_width: 38,
            history_height: 7,
            editor_command: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiffHunkRegion {
    pub rect: Rect,
    pub action: Option<Rect>,
    pub index: usize,
    pub continues_above: bool,
    pub continues_below: bool,
    pub scroll_start: usize,
    pub scroll_end: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Regions {
    pub screen: Option<Rect>,
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
    pub diff_scroll_max: usize,
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
    pub browser_overlay: Option<Rect>,
    pub browser_list: Option<Rect>,
    pub browser_tabs: [Option<Rect>; 3],
    pub command_overlay: Option<Rect>,
    pub command_output: Option<Rect>,
    pub editor_overlay: Option<Rect>,
    pub file_search_overlay: Option<Rect>,
    pub file_search_list: Option<Rect>,
    pub files_add: Option<Rect>,
    pub files_root: Option<Rect>,
    pub file_dialog_overlay: Option<Rect>,
    pub file_dialog_primary: Option<Rect>,
    pub file_dialog_secondary: Option<Rect>,
    pub editor_setting: Option<Rect>,
    pub auto_fetch: Option<Rect>,
    pub fetch_interval: Option<Rect>,
    pub fetch_interval_down: Option<Rect>,
    pub fetch_interval_up: Option<Rect>,
    pub stage_all: Option<Rect>,
    pub unstage_all: Option<Rect>,
    pub diff_hunks: Vec<DiffHunkRegion>,
}

pub struct App {
    pub(crate) session: RepositorySession,
    pub view: View,
    pub(crate) graph_commit_open: bool,
    pub mode: Mode,
    pub changes: ChangesState,
    pub graph_state: TableState,
    pub(crate) graph_scroll_to_selection: bool,
    pub(crate) commit_input: TextInput,
    pub dragging_splitter: bool,
    pub dragging_history: bool,
    pub dragging_diff_scrollbar: bool,
    diff_scroll_drag_offset: u16,
    pub picker: RepositoryPicker,
    pub(crate) file_search: FileSearch,
    pub(crate) actions: ActionsState,
    pub(crate) repository_browser: RepositoryBrowser,
    pub settings: Settings,
    pub settings_selection: usize,
    pub notice: Option<String>,
    pub regions: Regions,
    pub(crate) selection: SelectionState,
    copy_request: Option<String>,
    pub should_quit: bool,
    pub(crate) settings_path: Option<PathBuf>,
    pending_reload: Option<(changes::ChangesSelection, Option<String>)>,
    reload_queued: bool,
    pub(crate) editor_input: String,
    pub(crate) editor_error: Option<String>,
    pub(crate) editor_configure_only: bool,
    editor_request: Option<EditorRequest>,
    pub(crate) file_dialog: Option<FileDialog>,
    file_drag: Option<FileDrag>,
    pending_file_selection: Option<String>,
}

pub(crate) struct EditorRequest {
    pub(crate) command: Vec<String>,
    pub(crate) file: PathBuf,
    pub(crate) repository: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) enum FileDialogKind {
    Add {
        parent: String,
    },
    Name {
        action: FileNameAction,
        parent: String,
        source: Option<String>,
    },
    Delete {
        path: String,
        is_directory: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileNameAction {
    CreateFile,
    CreateDirectory,
    Rename,
}

pub(crate) struct FileDialog {
    pub(crate) kind: FileDialogKind,
    pub(crate) input: TextInput,
    pub(crate) choice: usize,
    pub(crate) error: Option<String>,
}

struct FileDrag {
    source: changes::ExplorerEntry,
    start: Position,
    active: bool,
    target: Option<String>,
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let settings_path = settings_path();
        let settings = settings_path
            .as_deref()
            .map(|path| {
                if path.exists() {
                    load_settings(path)
                } else {
                    legacy_settings_path()
                        .as_deref()
                        .map(load_settings)
                        .unwrap_or_default()
                }
            })
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
        let file_search = FileSearch::new(
            session.data().map_or(&[], |repo| repo.files.as_slice()),
            session.data().map(|repo| repo.files_fingerprint),
        );
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
            graph_commit_open: false,
            mode,
            changes,
            graph_state,
            graph_scroll_to_selection: true,
            commit_input: TextInput::default(),
            dragging_splitter: false,
            dragging_history: false,
            dragging_diff_scrollbar: false,
            diff_scroll_drag_offset: 0,
            picker: RepositoryPicker::new(start),
            file_search,
            actions: ActionsState::default(),
            repository_browser: RepositoryBrowser::default(),
            settings,
            settings_selection: 0,
            notice: None,
            regions: Regions::default(),
            selection: SelectionState::default(),
            copy_request: None,
            should_quit: false,
            settings_path,
            pending_reload: None,
            reload_queued: false,
            editor_input: String::new(),
            editor_error: None,
            editor_configure_only: false,
            editor_request: None,
            file_dialog: None,
            file_drag: None,
            pending_file_selection: None,
        }
    }

    pub(crate) fn repository(&self) -> Option<&RepositoryData> {
        self.session.data()
    }

    fn git_repository(&self) -> Option<&RepositoryData> {
        self.repository().filter(|repo| !repo.is_local())
    }

    fn require_git_repository(&mut self) -> bool {
        if self.git_repository().is_some() {
            return true;
        }
        self.notice = Some(
            if self.repository().is_some() {
                "Not a Git repository"
            } else {
                "Open a repository first"
            }
            .to_owned(),
        );
        false
    }

    pub(crate) fn commit_running(&self) -> bool {
        self.session.commit_running()
    }

    pub(crate) fn fetch_running(&self) -> bool {
        self.session.fetch_running()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.session.note_activity();
        if self.selection.has_selection() {
            self.selection.clear();
            if key.code == KeyCode::Esc {
                return;
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if key.code == KeyCode::F(3) && self.mode == Mode::Normal {
            self.open_file_search();
            return;
        }
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Commit => self.handle_commit_input(key),
            Mode::FileSearch => self.handle_file_search(key),
            Mode::Picker => self.handle_picker(key),
            Mode::Settings => self.handle_settings(key),
            Mode::RepositoryBrowser => self.handle_repository_browser(key),
            Mode::ActionMenu => self.handle_action_menu(key),
            Mode::Command => self.handle_command(key),
            Mode::Editor => self.handle_editor(key),
            Mode::Files => self.handle_file_dialog(key),
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Commit => self.commit_input.insert(text),
            Mode::FileSearch => {
                if let Some(repo) = self.session.data() {
                    self.file_search.paste(text, &repo.files);
                }
            }
            Mode::Picker => self.picker.paste(text),
            Mode::Command if self.actions.status != CommandStatus::Running => {
                self.actions.input.push_str(text);
                if self.actions.status == CommandStatus::Input {
                    self.actions.stderr.clear();
                }
            }
            Mode::Editor => {
                self.editor_input.push_str(text);
                self.editor_error = None;
            }
            Mode::Files => {
                if let Some(dialog) = &mut self.file_dialog
                    && matches!(dialog.kind, FileDialogKind::Name { .. })
                {
                    dialog.input.insert(text);
                    dialog.error = None;
                }
            }
            Mode::RepositoryBrowser => self.repository_browser.paste(text),
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
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

        if self.file_drag.is_some() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.update_file_drag(point),
                MouseEventKind::Up(MouseButton::Left) => self.finish_file_drag(point),
                _ => {}
            }
            return;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && !mouse.modifiers.contains(KeyModifiers::SHIFT)
            && self.begin_file_drag(point)
        {
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection.clear();
                if self.begin_mouse_control(point) {
                    return;
                }
                let region = self.selection_region(point);
                self.selection.begin(point, region);
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selection.is_active() => {
                self.selection.update(point);
                return;
            }
            MouseEventKind::Up(MouseButton::Left) if self.selection.is_active() => {
                match self.selection.finish(point) {
                    SelectionOutcome::Click => self.handle_left_click(point),
                    SelectionOutcome::Selected(Some(text)) => self.copy_request = Some(text),
                    SelectionOutcome::Selected(None) => {}
                }
                return;
            }
            _ => {}
        }

        if self.mode == Mode::ActionMenu {
            self.handle_action_mouse(mouse);
            return;
        }
        if self.mode == Mode::RepositoryBrowser {
            self.handle_repository_browser_mouse(mouse);
            return;
        }
        if self.mode == Mode::Command {
            self.handle_command_mouse(mouse);
            return;
        }
        if self.mode == Mode::Editor {
            return;
        }
        if self.mode == Mode::Files {
            return;
        }
        if self.mode == Mode::Picker {
            self.handle_picker_mouse(mouse);
            return;
        }
        if self.mode == Mode::FileSearch {
            self.handle_file_search_mouse(mouse);
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

        if mouse.kind == MouseEventKind::Moved {
            if self.select_graph_row(point) {
                return;
            }
            if self.changes.hunk_selection.is_some() {
                if let Some(hunk) = self
                    .regions
                    .diff_hunks
                    .iter()
                    .find(|hunk| hunk.rect.contains(point))
                {
                    self.changes.select_hunk(hunk.index);
                }
                return;
            }
        }

        if self.mode == Mode::Commit
            && mouse.kind == MouseEventKind::Down(MouseButton::Right)
            && !self.regions.commit.is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Normal;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => self.scroll_at(point, 1),
            MouseEventKind::ScrollUp => self.scroll_at(point, -1),
            MouseEventKind::Down(MouseButton::Right) if self.select_worktree_row(point) => {
                self.toggle_stage();
            }
            _ => {}
        }
    }

    pub fn take_copy_request(&mut self) -> Option<String> {
        self.copy_request.take()
    }

    fn begin_mouse_control(&mut self, point: Position) -> bool {
        if !matches!(self.mode, Mode::Normal | Mode::Commit) {
            return false;
        }
        if self
            .regions
            .splitter
            .is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Normal;
            self.dragging_splitter = true;
            self.resize_worktree(point.x);
            return true;
        }
        if self
            .regions
            .history_splitter
            .is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Normal;
            self.dragging_history = true;
            self.changes.history_focused = true;
            self.resize_history(point.y);
            return true;
        }
        if self
            .regions
            .diff_scrollbar
            .is_some_and(|rect| rect.contains(point))
            && self.regions.diff_scroll_max > 0
        {
            self.mode = Mode::Normal;
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
            self.scroll_diff_to(point.y);
            return true;
        }
        false
    }

    fn selection_region(&self, point: Position) -> Rect {
        [
            self.regions.command_overlay,
            self.regions.editor_overlay,
            self.regions.file_search_overlay,
            self.regions.file_dialog_overlay,
            self.regions.picker_overlay,
            self.regions.settings_overlay,
            self.regions.action_menu,
            self.regions.browser_overlay,
            self.regions.diff,
            self.regions.worktree,
            self.regions.graph_table,
        ]
        .into_iter()
        .flatten()
        .find(|region| region.contains(point))
        .or(self.regions.screen)
        .or_else(|| self.selection.screen_area())
        .unwrap_or(Rect::new(point.x, point.y, 1, 1))
    }

    fn handle_left_click(&mut self, point: Position) {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: point.x,
            row: point.y,
            modifiers: KeyModifiers::NONE,
        };
        match self.mode {
            Mode::ActionMenu => self.handle_action_mouse(mouse),
            Mode::Command => self.handle_command_mouse(mouse),
            Mode::Picker => self.handle_picker_mouse(mouse),
            Mode::FileSearch => self.handle_file_search_mouse(mouse),
            Mode::Settings => self.handle_settings_mouse(mouse),
            Mode::RepositoryBrowser => self.handle_repository_browser_mouse(mouse),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Editor => {}
            Mode::Files => self.handle_file_dialog_click(point),
            Mode::Normal | Mode::Commit => self.handle_primary_left_click(point),
        }
    }

    fn handle_primary_left_click(&mut self, point: Position) {
        if self.mode == Mode::Commit
            && !self.regions.commit.is_some_and(|rect| rect.contains(point))
        {
            self.mode = Mode::Normal;
        }
        if let Some(hunk) = self
            .regions
            .diff_hunks
            .iter()
            .find(|hunk| hunk.action.is_some_and(|rect| rect.contains(point)))
            .copied()
        {
            self.stage_hunk(hunk.index, false);
            return;
        }
        if self
            .regions
            .files_add
            .is_some_and(|rect| rect.contains(point))
        {
            self.open_add_dialog();
            return;
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
            self.graph_commit_open = false;
        } else if self.regions.graph.is_some_and(|rect| rect.contains(point)) {
            self.view = View::Graph;
            self.graph_commit_open = false;
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
            self.open_selected_graph_commit();
        } else if self.regions.commit.is_some_and(|rect| rect.contains(point)) {
            self.focus_commit();
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

    fn handle_repository_browser_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.repository_browser.move_selection(1),
            MouseEventKind::ScrollUp => self.repository_browser.move_selection(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .regions
                    .browser_overlay
                    .is_some_and(|rect| !rect.contains(point))
                {
                    self.mode = Mode::Normal;
                    return;
                }
                if let Some(tab) = self
                    .regions
                    .browser_tabs
                    .iter()
                    .position(|rect| rect.is_some_and(|rect| rect.contains(point)))
                {
                    self.repository_browser.set_tab(BrowserTab::ALL[tab]);
                    return;
                }
                let Some(list) = self
                    .regions
                    .browser_list
                    .filter(|rect| rect.contains(point))
                else {
                    return;
                };
                let index = self.repository_browser.state.offset() + usize::from(point.y - list.y);
                self.repository_browser.select(index);
            }
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

    fn handle_file_search_mouse(&mut self, mouse: MouseEvent) {
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.file_search.move_selection(1),
            MouseEventKind::ScrollUp => self.file_search.move_selection(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .regions
                    .file_search_overlay
                    .is_some_and(|rect| !rect.contains(point))
                {
                    self.mode = Mode::Normal;
                    return;
                }
                let Some(list) = self
                    .regions
                    .file_search_list
                    .filter(|rect| rect.contains(point))
                else {
                    return;
                };
                let index = self.file_search.state.offset() + usize::from(point.y - list.y);
                if self.file_search.select(index) {
                    self.activate_file_search_result();
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
        } else if self
            .regions
            .editor_setting
            .is_some_and(|rect| rect.contains(point))
        {
            self.settings_selection = 2;
            self.open_editor_setting();
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
        self.graph_scroll_to_selection = false;
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
            self.scroll_graph(delta.saturating_mul(3));
        }
    }

    fn scroll_graph(&mut self, delta: isize) {
        let viewport = self
            .regions
            .graph_table
            .map_or(0, |rect| usize::from(rect.height));
        let len = self.repository().map_or(0, |repo| repo.commits.len());
        scroll_table(&mut self.graph_state, len, viewport, delta);
        self.graph_scroll_to_selection = false;
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

    pub fn poll_worker(&mut self) -> bool {
        let mut changed = self.mode == Mode::Picker && self.picker.poll_index();
        changed |= self.repository_browser.poll();
        changed |= self.commit_input.poll_blink(self.mode == Mode::Commit);
        if let Some(dialog) = &mut self.file_dialog {
            changed |= dialog.input.poll_blink(
                self.mode == Mode::Files && matches!(dialog.kind, FileDialogKind::Name { .. }),
            );
        }
        let interval = fetch_interval(&self.settings);
        self.session
            .maybe_start_fetch(self.settings.auto_fetch, interval);
        self.session.maybe_start_status_check();
        while let Some(done) = self.session.next_worker_completion(interval) {
            changed = true;
            match done {
                WorkerCompletion::Commit(result) => match result {
                    Ok(output) if output.success => {
                        self.commit_input.clear();
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
                WorkerCompletion::Mutation(result) => match result {
                    Ok(()) => self.reload(),
                    Err(error) => {
                        self.changes.cancel_pending_hunk_stage();
                        self.notice = Some(error);
                    }
                },
                WorkerCompletion::FileOperation(done) => match done.result {
                    Ok(selection) => {
                        self.pending_file_selection = selection;
                        self.reload();
                        self.notice = Some(done.message);
                    }
                    Err(error) => self.notice = Some(error),
                },
            }
        }
        while self.session.next_worktree_change() {
            changed = true;
            self.reload();
            self.notice = None;
        }
        while let Some(done) = self.session.next_load_completion() {
            changed = true;
            match (done.kind, done.result) {
                (LoadKind::Open, Ok(())) => {
                    self.pending_reload = None;
                    self.pending_file_selection = None;
                    self.reload_queued = false;
                    self.mode = Mode::Normal;
                    self.actions = ActionsState::default();
                    self.notice = Some(
                        if self.session.data().is_some_and(RepositoryData::is_local) {
                            "Workspace opened"
                        } else {
                            "Repository opened"
                        }
                        .to_owned(),
                    );
                    self.graph_state = TableState::default();
                    self.graph_scroll_to_selection = true;
                    self.changes.reset_repository(self.session.data());
                    self.file_search.reindex(
                        self.session
                            .data()
                            .map_or(&[], |repo| repo.files.as_slice()),
                        self.session.data().map(|repo| repo.files_fingerprint),
                    );
                    self.graph_state.select(
                        self.session
                            .data()
                            .is_some_and(|repo| !repo.commits.is_empty())
                            .then_some(0),
                    );
                }
                (LoadKind::Open, Err(error)) => {
                    self.notice = None;
                    self.picker.error = Some(error);
                }
                (LoadKind::Reload, Ok(())) => {
                    if let Some((selection, selected_oid)) = self.pending_reload.take() {
                        let repo = self.session.data().expect("reloaded repository");
                        let commit_index = selected_oid.and_then(|oid| {
                            repo.commits.iter().position(|commit| commit.oid == oid)
                        });
                        self.graph_state
                            .select(commit_index.or_else(|| repo.commits.first().map(|_| 0)));
                        self.graph_scroll_to_selection = true;
                        self.changes.restore_selection(repo, selection);
                        if let Some(path) = self.pending_file_selection.take() {
                            let viewport = self
                                .regions
                                .explorer_list
                                .map_or(0, |rect| usize::from(rect.height));
                            self.changes.select_explorer_path(repo, &path, viewport);
                        }
                    }
                    if let Some(repo) = self.session.data() {
                        self.file_search
                            .reindex(&repo.files, Some(repo.files_fingerprint));
                    }
                    if self.notice.as_deref() == Some("Refreshing…") {
                        self.notice = Some("Refreshed".to_owned());
                    }
                    if self.reload_queued {
                        self.reload_queued = false;
                        self.reload();
                    }
                }
                (LoadKind::Reload, Err(error)) => {
                    self.pending_reload = None;
                    self.reload_queued = false;
                    self.notice = Some(error);
                }
            }
        }
        changed |= self
            .changes
            .poll_preview(self.session.data().map(|repo| repo.root.as_path()));
        changed
    }

    pub fn requires_render_before_next_event(&self) -> bool {
        self.editor_request.is_some()
            || self.changes.hunk_selection.is_some()
            || self
                .regions
                .screen
                .is_some_and(|area| self.selection.needs_capture(area))
    }

    pub fn change_counts(&self) -> (usize, usize) {
        self.repository().map_or((0, 0), |repo| repo.change_counts)
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Esc | KeyCode::Tab)
            && self.view == View::Graph
            && self.graph_commit_open
        {
            self.graph_commit_open = false;
            return;
        }
        if key.code == KeyCode::Char('f') && self.view == View::Graph {
            self.view = View::Changes;
            self.graph_commit_open = false;
            self.set_left_pane(LeftPane::Files);
            return;
        }
        if let Some(index) = self.changes.hunk_selection {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                    self.changes.leave_hunk_selection();
                }
                KeyCode::Down | KeyCode::Char('j') => self.scroll_or_move_hunk(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll_or_move_hunk(-1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
                    self.stage_hunk(index, true);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') if self.commit_running() || self.session.command_running() => {
                self.notice = Some("A Git operation is still running".to_owned())
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => {
                self.view = View::Changes;
                self.graph_commit_open = false;
            }
            KeyCode::Char('2') => {
                self.view = View::Graph;
                self.graph_commit_open = false;
            }
            KeyCode::Tab => {
                self.view = match self.view {
                    View::Changes => View::Graph,
                    View::Graph => View::Changes,
                };
                self.graph_commit_open = false;
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('o') => self.open_picker(),
            KeyCode::Char('s') => self.mode = Mode::Settings,
            KeyCode::Char('b') => self.open_repository_browser(),
            KeyCode::Char('x') => self.open_actions(),
            KeyCode::Char('g') => self.open_git_command(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('w') if self.view == View::Changes || self.graph_commit_open => {
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
            KeyCode::F(2) if self.view == View::Changes && self.changes.pane == LeftPane::Files => {
                self.open_rename_dialog();
            }
            KeyCode::Delete
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.view == View::Changes
                    && self.changes.pane == LeftPane::Files =>
            {
                self.open_delete_dialog();
            }
            KeyCode::Char('e') if self.view == View::Changes => self.open_selected_file(false),
            KeyCode::Char('E') if self.view == View::Changes => self.open_selected_file(true),
            KeyCode::Char('f') if self.view == View::Changes => self.toggle_left_pane(),
            KeyCode::Char('c') if self.view == View::Changes => {
                self.set_left_pane(LeftPane::Worktree);
                self.focus_commit();
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
            KeyCode::Enter if self.view == View::Graph && !self.graph_commit_open => {
                self.open_selected_graph_commit();
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
                if !repo.is_some_and(|repo| self.changes.enter_hunk_selection(repo)) {
                    self.changes.expand_or_descend_worktree(repo);
                }
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
            KeyCode::PageDown if self.view == View::Changes || self.graph_commit_open => {
                self.scroll_diff_by(10)
            }
            KeyCode::PageUp if self.view == View::Changes || self.graph_commit_open => {
                self.scroll_diff_by(-10)
            }
            KeyCode::Down | KeyCode::Char('j') if self.graph_commit_open => self.scroll_diff_by(1),
            KeyCode::Up | KeyCode::Char('k') if self.graph_commit_open => self.scroll_diff_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Home => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_last(),
            _ => {}
        }
    }

    fn scroll_or_move_hunk(&mut self, delta: isize) {
        let Some(selected) = self.changes.hunk_selection else {
            return;
        };
        let Some(region) = self
            .regions
            .diff_hunks
            .iter()
            .find(|region| region.index == selected)
        else {
            self.changes.move_hunk_selection(delta);
            return;
        };
        let can_scroll = if delta > 0 {
            region.continues_below
        } else {
            region.continues_above
        };
        if can_scroll {
            if delta > 0 {
                self.changes.diff_scroll = self
                    .changes
                    .diff_scroll
                    .saturating_add(10)
                    .min(region.scroll_end);
            } else {
                self.changes.diff_scroll = self
                    .changes
                    .diff_scroll
                    .saturating_sub(10)
                    .max(region.scroll_start);
            }
        } else {
            self.changes.move_hunk_selection(delta);
        }
    }

    fn handle_commit_input(&mut self, key: KeyEvent) {
        self.commit_input.focus();
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_commit();
            }
            KeyCode::Char('j' | 'm') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_commit();
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.commit_input.select_all();
            }
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.commit_input.delete_word();
            }
            KeyCode::Left => self.commit_input.move_left(),
            KeyCode::Right => self.commit_input.move_right(),
            KeyCode::Home => self.commit_input.move_home(),
            KeyCode::End => self.commit_input.move_end(),
            KeyCode::Delete => self.commit_input.delete(),
            KeyCode::Enter => self.commit_input.insert_char('\n'),
            KeyCode::Backspace => self.commit_input.backspace(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.commit_input.insert_char(character);
            }
            _ => {}
        }
    }

    fn handle_picker(&mut self, key: KeyEvent) {
        let command = self.picker.handle_key(key, self.repository().is_some());
        self.apply_picker_command(command);
    }

    fn handle_file_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::F(3) => self.mode = Mode::Normal,
            KeyCode::Enter => self.activate_file_search_result(),
            KeyCode::Down | KeyCode::Tab => self.file_search.move_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.file_search.move_selection(-1),
            KeyCode::Backspace => {
                if let Some(repo) = self.session.data() {
                    self.file_search.backspace(&repo.files);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(repo) = self.session.data() {
                    self.file_search.clear(&repo.files);
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(repo) = self.session.data() {
                    self.file_search.push(character, &repo.files);
                }
            }
            _ => {}
        }
    }

    fn handle_settings(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('s') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.settings_selection = (self.settings_selection + 1) % 3;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.settings_selection = (self.settings_selection + 2) % 3;
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
            KeyCode::Enter | KeyCode::Char(' ') if self.settings_selection == 2 => {
                self.open_editor_setting();
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

    fn handle_editor(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.editor_error = None;
                self.mode = if self.editor_configure_only {
                    Mode::Settings
                } else {
                    Mode::Normal
                };
            }
            KeyCode::Enter => self.queue_editor(),
            KeyCode::Backspace => {
                self.editor_input.pop();
                self.editor_error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_input.clear();
                self.editor_error = None;
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_input.push(character);
                self.editor_error = None;
            }
            _ => {}
        }
    }

    fn handle_file_dialog(&mut self, key: KeyEvent) {
        let Some(kind) = self.file_dialog.as_ref().map(|dialog| dialog.kind.clone()) else {
            self.mode = Mode::Normal;
            return;
        };
        match kind {
            FileDialogKind::Add { parent } => match key.code {
                KeyCode::Esc => self.close_file_dialog(),
                KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                    if let Some(dialog) = &mut self.file_dialog {
                        dialog.choice = 0;
                    }
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                    if let Some(dialog) = &mut self.file_dialog {
                        dialog.choice = 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let action = if self
                        .file_dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.choice == 1)
                    {
                        FileNameAction::CreateDirectory
                    } else {
                        FileNameAction::CreateFile
                    };
                    self.open_name_dialog(action, parent, None);
                }
                _ => {}
            },
            FileDialogKind::Name { .. } => {
                let Some(dialog) = &mut self.file_dialog else {
                    return;
                };
                dialog.input.focus();
                match key.code {
                    KeyCode::Esc => self.close_file_dialog(),
                    KeyCode::Enter => self.submit_file_name(),
                    KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        dialog.input.select_all();
                    }
                    KeyCode::Backspace
                        if key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        dialog.input.delete_word();
                        dialog.error = None;
                    }
                    KeyCode::Left => dialog.input.move_left(),
                    KeyCode::Right => dialog.input.move_right(),
                    KeyCode::Home => dialog.input.move_home(),
                    KeyCode::End => dialog.input.move_end(),
                    KeyCode::Delete => dialog.input.delete(),
                    KeyCode::Backspace => dialog.input.backspace(),
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        dialog.input.insert_char(character);
                        dialog.error = None;
                    }
                    _ => {}
                }
            }
            FileDialogKind::Delete { .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => self.close_file_dialog(),
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_delete(),
                _ => {}
            },
        }
    }

    fn open_add_dialog(&mut self) {
        let parent = self
            .session
            .data()
            .and_then(|repo| self.changes.selected_explorer_entry(repo))
            .map_or_else(String::new, |entry| {
                if entry.is_directory {
                    entry.path
                } else {
                    relative_parent(&entry.path)
                }
            });
        self.file_dialog = Some(FileDialog {
            kind: FileDialogKind::Add { parent },
            input: TextInput::default(),
            choice: 0,
            error: None,
        });
        self.mode = Mode::Files;
    }

    fn open_rename_dialog(&mut self) {
        let Some(entry) = self
            .session
            .data()
            .and_then(|repo| self.changes.selected_explorer_entry(repo))
        else {
            self.notice = Some("Select a file or folder to rename".to_owned());
            return;
        };
        let name = Path::new(&entry.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&entry.path)
            .to_owned();
        self.open_name_dialog(
            FileNameAction::Rename,
            relative_parent(&entry.path),
            Some(entry.path),
        );
        if let Some(dialog) = &mut self.file_dialog {
            dialog.input.insert(&name);
            dialog.input.select_all();
        }
    }

    fn open_delete_dialog(&mut self) {
        let Some(entry) = self
            .session
            .data()
            .and_then(|repo| self.changes.selected_explorer_entry(repo))
        else {
            self.notice = Some("Select a file or folder to delete".to_owned());
            return;
        };
        self.file_dialog = Some(FileDialog {
            kind: FileDialogKind::Delete {
                path: entry.path,
                is_directory: entry.is_directory,
            },
            input: TextInput::default(),
            choice: 0,
            error: None,
        });
        self.mode = Mode::Files;
    }

    fn open_name_dialog(&mut self, action: FileNameAction, parent: String, source: Option<String>) {
        let mut input = TextInput::default();
        input.focus();
        self.file_dialog = Some(FileDialog {
            kind: FileDialogKind::Name {
                action,
                parent,
                source,
            },
            input,
            choice: 0,
            error: None,
        });
        self.mode = Mode::Files;
    }

    fn submit_file_name(&mut self) {
        let Some(dialog) = &self.file_dialog else {
            return;
        };
        let FileDialogKind::Name {
            action,
            parent,
            source,
        } = dialog.kind.clone()
        else {
            return;
        };
        let name = dialog.input.text().to_owned();
        if let Err(error) = validate_name(&name) {
            if let Some(dialog) = &mut self.file_dialog {
                dialog.error = Some(error.to_string());
            }
            return;
        }
        let destination = join_relative(&parent, &name);
        let operation = match action {
            FileNameAction::CreateFile => FileOperation::CreateFile { path: destination },
            FileNameAction::CreateDirectory => FileOperation::CreateDirectory { path: destination },
            FileNameAction::Rename => {
                let Some(source) = source else { return };
                if source == destination {
                    self.close_file_dialog();
                    return;
                }
                FileOperation::Rename {
                    from: source,
                    to: destination,
                }
            }
        };
        self.close_file_dialog();
        self.start_file_operation(operation);
    }

    fn confirm_delete(&mut self) {
        let Some(FileDialogKind::Delete { path, .. }) =
            self.file_dialog.as_ref().map(|dialog| dialog.kind.clone())
        else {
            return;
        };
        self.close_file_dialog();
        self.start_file_operation(FileOperation::Delete { path });
    }

    fn close_file_dialog(&mut self) {
        self.file_dialog = None;
        self.mode = Mode::Normal;
    }

    fn start_file_operation(&mut self, operation: FileOperation) {
        if !self.session.start_file_operation(operation) {
            self.notice = Some("Another repository operation is running".to_owned());
        }
    }

    fn handle_file_dialog_click(&mut self, point: Position) {
        if self
            .regions
            .file_dialog_primary
            .is_some_and(|rect| rect.contains(point))
        {
            match self.file_dialog.as_ref().map(|dialog| dialog.kind.clone()) {
                Some(FileDialogKind::Add { parent }) => {
                    self.open_name_dialog(FileNameAction::CreateFile, parent, None);
                }
                Some(FileDialogKind::Name { .. }) => self.submit_file_name(),
                Some(FileDialogKind::Delete { .. }) => self.confirm_delete(),
                None => {}
            }
        } else if self
            .regions
            .file_dialog_secondary
            .is_some_and(|rect| rect.contains(point))
        {
            match self.file_dialog.as_ref().map(|dialog| dialog.kind.clone()) {
                Some(FileDialogKind::Add { parent }) => {
                    self.open_name_dialog(FileNameAction::CreateDirectory, parent, None);
                }
                _ => self.close_file_dialog(),
            }
        } else if matches!(
            self.file_dialog.as_ref().map(|dialog| &dialog.kind),
            Some(FileDialogKind::Add { .. })
        ) && !self
            .regions
            .file_dialog_overlay
            .is_some_and(|rect| rect.contains(point))
        {
            self.close_file_dialog();
        }
    }

    fn begin_file_drag(&mut self, point: Position) -> bool {
        if self.mode != Mode::Normal
            || self.view != View::Changes
            || self.changes.pane != LeftPane::Files
        {
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
        let Some(source) = self.changes.explorer_entry(repo, index) else {
            return false;
        };
        self.file_drag = Some(FileDrag {
            source,
            start: point,
            active: false,
            target: None,
        });
        true
    }

    fn update_file_drag(&mut self, point: Position) {
        let mut target = self.file_drop_target_at(point);
        if let Some(drag) = &mut self.file_drag {
            drag.active |= drag.start != point;
            if drag.source.is_directory && target.as_deref() == Some(&drag.source.path) {
                target = None;
            }
            drag.target = target;
        }
    }

    fn finish_file_drag(&mut self, point: Position) {
        self.update_file_drag(point);
        let Some(drag) = self.file_drag.take() else {
            return;
        };
        if !drag.active {
            self.handle_primary_left_click(point);
            return;
        }
        let Some(target) = drag.target else {
            return;
        };
        let Some(name) = Path::new(&drag.source.path)
            .file_name()
            .and_then(|name| name.to_str())
        else {
            self.notice = Some("Could not determine the entry name".to_owned());
            return;
        };
        let destination = join_relative(&target, name);
        if destination == drag.source.path {
            return;
        }
        self.start_file_operation(FileOperation::Move {
            from: drag.source.path,
            to: destination,
        });
    }

    fn file_drop_target_at(&self, point: Position) -> Option<String> {
        if self
            .regions
            .files_root
            .is_some_and(|rect| rect.contains(point))
        {
            return Some(String::new());
        }
        let rect = self
            .regions
            .explorer_list
            .filter(|rect| rect.contains(point))?;
        let index = self.changes.explorer_scroll + usize::from(point.y - rect.y);
        let repo = self.session.data()?;
        let entry = self.changes.explorer_entry(repo, index)?;
        entry.is_directory.then_some(entry.path)
    }

    pub(crate) fn file_drop_target(&self) -> Option<&str> {
        self.file_drag
            .as_ref()
            .filter(|drag| drag.active)
            .and_then(|drag| drag.target.as_deref())
    }

    fn open_selected_file(&mut self, configure: bool) {
        let Some((repository, file)) = self.selected_file_to_edit() else {
            self.notice = Some("Select a file to edit".to_owned());
            return;
        };
        let configured = self.settings.editor_command.clone();
        if configure || configured.is_none() {
            self.editor_configure_only = false;
            self.editor_input = configured
                .or_else(|| std::env::var("VISUAL").ok())
                .or_else(|| std::env::var("EDITOR").ok())
                .unwrap_or_default();
            self.editor_error = None;
            self.mode = Mode::Editor;
            return;
        }
        self.prepare_editor_request(configured.expect("checked above"), repository, file);
    }

    fn open_selected_graph_commit(&mut self) {
        let Some(commit) = self
            .graph_state
            .selected()
            .and_then(|index| self.session.data()?.commits.get(index))
            .cloned()
        else {
            return;
        };
        let Some(repo) = self.session.data() else {
            return;
        };
        self.changes.preview_commit(repo, &commit);
        self.graph_commit_open = true;
    }

    fn queue_editor(&mut self) {
        if self.editor_configure_only {
            let command = self.editor_input.trim().to_owned();
            match parse_command_args(&command) {
                Ok(_) => {
                    self.settings.editor_command = Some(command);
                    self.persist_settings();
                    self.editor_error = None;
                    self.mode = Mode::Settings;
                }
                Err(error) => self.editor_error = Some(error),
            }
            return;
        }
        let Some((repository, file)) = self.selected_file_to_edit() else {
            self.mode = Mode::Normal;
            self.notice = Some("Select a file to edit".to_owned());
            return;
        };
        self.prepare_editor_request(self.editor_input.trim().to_owned(), repository, file);
    }

    fn open_editor_setting(&mut self) {
        self.editor_input = self
            .settings
            .editor_command
            .clone()
            .or_else(|| std::env::var("VISUAL").ok())
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_default();
        self.editor_error = None;
        self.editor_configure_only = true;
        self.mode = Mode::Editor;
    }

    fn prepare_editor_request(&mut self, command: String, repository: PathBuf, file: PathBuf) {
        match parse_command_args(&command) {
            Ok(command_args) => {
                self.settings.editor_command = Some(command);
                self.persist_settings();
                self.editor_error = None;
                self.editor_request = Some(EditorRequest {
                    command: command_args,
                    file: repository.join(file),
                    repository,
                });
                self.mode = Mode::Normal;
            }
            Err(error) => {
                self.editor_error = Some(error);
                self.mode = Mode::Editor;
            }
        }
    }

    fn selected_file_to_edit(&self) -> Option<(PathBuf, PathBuf)> {
        if self.view != View::Changes || self.changes.history_focused {
            return None;
        }
        let repo = self.repository()?;
        let path = match self.changes.pane {
            LeftPane::Worktree => {
                let index = self.changes.selected_change_index(repo)?;
                repo.changes.get(index)?.path.as_str()
            }
            LeftPane::Files => self.changes.selected_explorer_file_path(repo)?,
        };
        Some((repo.root.clone(), PathBuf::from(path)))
    }

    pub(crate) fn take_editor_request(&mut self) -> Option<EditorRequest> {
        self.editor_request.take()
    }

    pub(crate) fn editor_finished(&mut self, result: Result<(), String>) {
        let error = result.err();
        self.reload();
        if let Some(error) = error {
            self.notice = Some(error);
        }
    }

    fn open_actions(&mut self) {
        if self.require_git_repository() {
            self.mode = Mode::ActionMenu;
        }
    }

    fn open_repository_browser(&mut self) {
        let Some(repo) = self.git_repository() else {
            self.require_git_repository();
            return;
        };
        let root = repo.root.clone();
        let branches = repo.branches.clone();
        self.repository_browser.open(&root, &branches);
        self.mode = Mode::RepositoryBrowser;
    }

    fn handle_repository_browser(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Tab | KeyCode::Right => self.repository_browser.move_tab(1),
            KeyCode::BackTab | KeyCode::Left => self.repository_browser.move_tab(-1),
            KeyCode::Down => self.repository_browser.move_selection(1),
            KeyCode::Up => self.repository_browser.move_selection(-1),
            KeyCode::Home => self.repository_browser.state.select(Some(0)),
            KeyCode::End => {
                let count = self.repository_browser.result_count();
                self.repository_browser.state.select(count.checked_sub(1));
            }
            KeyCode::Backspace => self.repository_browser.backspace(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.repository_browser.clear();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.repository_browser.push(character);
            }
            _ => {}
        }
    }

    fn open_git_command(&mut self) {
        if self.require_git_repository() {
            self.actions.begin_input();
            self.mode = Mode::Command;
        }
    }

    fn activate_action(&mut self) {
        let action = self.actions.selected();
        if action == ActionId::Commit {
            self.view = View::Changes;
            self.set_left_pane(LeftPane::Worktree);
            if self.commit_input.text().trim().is_empty() {
                self.focus_commit();
            } else {
                self.start_commit();
            }
            return;
        }
        if action == ActionId::Custom {
            self.open_git_command();
            return;
        }
        if let Some((label, args)) = action_command(action) {
            self.start_git_command(label.to_owned(), args);
        }
    }

    fn start_git_command(&mut self, label: String, args: Vec<String>) {
        if !self.require_git_repository() {
            self.mode = Mode::Normal;
            return;
        }
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
        if self
            .session
            .start_open(path, fetch_interval(&self.settings))
        {
            self.picker.error = None;
            self.notice = Some("Opening repository…".to_owned());
        } else {
            self.picker.error = Some("Another workspace operation is running".to_owned());
        }
    }

    fn open_picker(&mut self) {
        let start = self
            .repository()
            .map(|repo| repo.root.clone())
            .unwrap_or_else(|| self.picker.directory.clone());
        if self.picker.directory == start {
            let _ = self.picker.poll_index();
        } else {
            self.picker.navigate(start);
        }
        self.picker.editing_path = false;
        self.mode = Mode::Picker;
    }

    fn open_file_search(&mut self) {
        if self.repository().is_none() {
            return;
        }
        self.file_search.open();
        self.mode = Mode::FileSearch;
    }

    fn activate_file_search_result(&mut self) {
        let Some(file_index) = self.file_search.selected_file_index() else {
            return;
        };
        let viewport = self
            .regions
            .explorer_list
            .map_or(0, |rect| usize::from(rect.height));
        let Some(repo) = self.session.data() else {
            return;
        };
        self.changes.set_pane(LeftPane::Files, Some(repo));
        if self
            .changes
            .select_explorer_file(repo, file_index, viewport)
        {
            self.view = View::Changes;
            self.graph_commit_open = false;
            self.mode = Mode::Normal;
        }
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
                self.graph_scroll_to_selection = true;
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
            View::Graph => {
                self.graph_state.select(
                    self.repository()
                        .is_some_and(|repo| !repo.commits.is_empty())
                        .then_some(0),
                );
                self.graph_scroll_to_selection = true;
            }
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
            View::Graph => {
                self.graph_state.select(
                    self.repository()
                        .and_then(|repo| repo.commits.len().checked_sub(1)),
                );
                self.graph_scroll_to_selection = true;
            }
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
        let mutation = if change.staged {
            Mutation::Unstage(change)
        } else {
            Mutation::Stage(change)
        };
        let _ = self.session.start_mutation(mutation);
    }

    fn stage_hunk(&mut self, index: usize, preserve_selection: bool) {
        let patch = self.changes.diff.clone();
        let path = preserve_selection
            .then(|| {
                let repo = self.repository()?;
                let index = self.changes.selected_change_index(repo)?;
                Some(repo.changes.get(index)?.path.clone())
            })
            .flatten();
        let started = self
            .session
            .start_mutation(Mutation::StageHunk { patch, index });
        if started && let Some(path) = path {
            self.changes
                .preserve_hunk_selection_after_stage(path, index);
        }
    }

    fn stage_all(&mut self) {
        if self.require_git_repository() {
            let _ = self.session.start_mutation(Mutation::StageAll);
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
        if self.require_git_repository() {
            let _ = self.session.start_mutation(Mutation::UnstageAll);
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

        if self.session.start_reload(fetch_interval(&self.settings)) {
            self.pending_reload = Some((selection, selected_oid));
            self.notice = Some("Refreshing…".to_owned());
        } else {
            self.pending_reload = Some((selection, selected_oid));
            self.reload_queued = true;
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
        if !self.require_git_repository() {
            self.mode = Mode::Normal;
            return;
        }
        if self.session.commit_running() {
            self.notice = Some("A commit is already running".to_owned());
            return;
        }
        if self.session.command_running() {
            self.notice = Some("Another Git operation is already running".to_owned());
            return;
        }
        let message = self.commit_input.text().trim().to_owned();
        if message.is_empty() {
            self.notice = Some("Commit message cannot be empty".to_owned());
            return;
        }
        if self.session.start_commit(message) {
            self.mode = Mode::Normal;
        }
    }

    fn focus_commit(&mut self) {
        if !self.require_git_repository() {
            return;
        }
        self.mode = Mode::Commit;
        self.commit_input.focus();
    }
}

fn fetch_interval(settings: &Settings) -> Duration {
    Duration::from_secs(u64::from(settings.fetch_interval_minutes) * 60)
}

fn settings_path() -> Option<PathBuf> {
    config_path("hunkle")
}

fn legacy_settings_path() -> Option<PathBuf> {
    config_path("gitui")
}

fn config_path(app_name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join(app_name).join("config"));
    }
    if let Some(path) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(path).join(app_name).join("config"));
    }
    home_directory().map(|home| home.join(".config").join(app_name).join("config"))
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
            "editor_command" => {
                let command = value.trim();
                settings.editor_command = (!command.is_empty()).then(|| command.to_owned());
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
            "auto_fetch={}\nfetch_interval_minutes={}\nworktree_width={}\nhistory_height={}\neditor_command={}\n",
            settings.auto_fetch,
            settings.fetch_interval_minutes,
            settings.worktree_width,
            settings.history_height,
            settings.editor_command.as_deref().unwrap_or_default()
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

fn relative_parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn scroll_table(state: &mut TableState, len: usize, viewport: usize, delta: isize) {
    let maximum = len.saturating_sub(viewport);
    *state.offset_mut() = state.offset().saturating_add_signed(delta).min(maximum);
}

#[cfg(test)]
mod tests {
    use std::{process::Command, thread};

    use super::*;

    #[test]
    fn graph_scrolling_moves_the_viewport_without_moving_selection() {
        let mut state = TableState::default().with_selected(4);

        scroll_table(&mut state, 20, 5, 3);
        assert_eq!(state.selected(), Some(4));
        assert_eq!(state.offset(), 3);

        scroll_table(&mut state, 20, 5, 30);
        assert_eq!(state.selected(), Some(4));
        assert_eq!(state.offset(), 15);
    }

    #[test]
    fn opens_a_nested_directory_as_a_local_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let nested = root.join("nested/config");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("settings.toml"), "theme = 'test'\n").unwrap();

        let app = App::new(nested.clone());

        let repo = app.session.data().unwrap();
        assert!(repo.is_local());
        assert_eq!(repo.root, fs::canonicalize(&nested).unwrap());
        assert_eq!(repo.branch, "local");
        assert_eq!(repo.files, ["settings.toml"]);
        assert_eq!(app.change_counts(), (0, 0));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.changes.pane, LeftPane::Files);
    }

    #[test]
    fn local_workspaces_reload_files_and_reject_git_actions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("one.txt"), "one\n").unwrap();
        let mut app = App::new(root.to_path_buf());
        assert!(app.repository().unwrap().is_local());

        for key in ['x', 'g', 'c', 'a', 'u', 'b'] {
            app.mode = Mode::Normal;
            app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
            assert_eq!(app.mode, Mode::Normal, "{key}");
            assert_eq!(app.notice.as_deref(), Some("Not a Git repository"), "{key}");
        }

        fs::write(root.join("two.txt"), "two\n").unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        for _ in 0..100 {
            let _ = app.poll_worker();
            if app.repository().unwrap().files == ["one.txt", "two.txt"] {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.repository().unwrap().files, ["one.txt", "two.txt"]);
    }

    #[test]
    fn creates_renames_drags_and_deletes_files_from_the_files_pane() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("old.txt"), "content\n").unwrap();
        fs::create_dir(root.join("destination")).unwrap();
        let mut app = App::new(root.to_path_buf());
        app.view = View::Changes;
        app.changes.pane = LeftPane::Files;

        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Files);
        app.handle_paste(" renamed.txt ");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        wait_for_state(&mut app, |app| {
            app.repository()
                .is_some_and(|repo| repo.files.iter().any(|path| path == " renamed.txt "))
        });
        assert!(root.join(" renamed.txt ").is_file());
        assert_eq!(app.selected_explorer_file_path(), Some(" renamed.txt "));

        app.open_add_dialog();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_paste("created");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        wait_for_state(&mut app, |app| {
            app.repository()
                .is_some_and(|repo| repo.directories.iter().any(|path| path == "created"))
        });
        assert!(root.join("created").is_dir());

        let repo = app.session.data().unwrap();
        assert!(app.changes.select_explorer_path(repo, " renamed.txt ", 20));
        app.regions.explorer_list = Some(Rect::new(0, 10, 30, 20));
        let source = app
            .changes
            .explorer_rows()
            .iter()
            .position(|row| {
                row.file_index
                    .and_then(|index| app.repository().unwrap().files.get(index))
                    .is_some_and(|path| path == " renamed.txt ")
            })
            .unwrap();
        let target = app
            .changes
            .explorer_rows()
            .iter()
            .position(|row| row.directory_path.as_deref() == Some("created"))
            .unwrap();
        assert!(app.begin_file_drag(Position::new(1, 10 + source as u16)));
        app.update_file_drag(Position::new(1, 10 + target as u16));
        app.finish_file_drag(Position::new(1, 10 + target as u16));
        wait_for_state(&mut app, |app| {
            app.repository().is_some_and(|repo| {
                repo.files
                    .iter()
                    .any(|path| path == "created/ renamed.txt ")
            })
        });
        assert!(root.join("created/ renamed.txt ").is_file());

        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        assert!(matches!(
            app.file_dialog.as_ref().map(|dialog| &dialog.kind),
            Some(FileDialogKind::Delete { .. })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(root.join("created/ renamed.txt ").is_file());
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        wait_for_state(&mut app, |_| !root.join("created/ renamed.txt ").exists());
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
            editor_command: Some("code --wait".to_owned()),
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
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.graph_commit_open = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(app.view, View::Changes);
        assert_eq!(app.changes.pane, LeftPane::Files);
        assert!(!app.graph_commit_open);
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(app.changes.pane, LeftPane::Worktree);
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(app.changes.pane, LeftPane::Files);

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
                editor_command: None,
            }
        );
        assert_eq!(load_settings(&path), app.settings);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert!(app.editor_configure_only);
        app.editor_input.clear();
        app.handle_paste("nvim");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Settings);
        assert_eq!(app.settings.editor_command.as_deref(), Some("nvim"));
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
        let _ = app.poll_worker();
        assert!(app.fetch_running());

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            let _ = app.poll_worker();
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
        app.commit_input.set("commit from control enter");
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(app.commit_running());
        assert_eq!(app.commit_input.text(), "commit from control enter");

        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            let _ = app.poll_worker();
            if app.repository().unwrap().commits.len() == 2 {
                break;
            }
        }
        assert!(!app.commit_running());
        assert!(app.commit_input.is_empty());
        assert_eq!(app.repository().unwrap().commits.len(), 2);
    }

    #[test]
    fn commit_action_submits_an_existing_message() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        fs::write(root.join("next.txt"), "next\n").unwrap();
        run_git(root, &["add", "next.txt"]);

        let mut app = App::new(root.to_path_buf());
        app.commit_input.set("commit from actions");
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.commit_running());
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            let _ = app.poll_worker();
            if app.repository().unwrap().commits.len() == 2 {
                break;
            }
        }
        assert!(!app.commit_running());
        assert!(app.commit_input.is_empty());
        assert_eq!(app.repository().unwrap().commits.len(), 2);
    }

    #[test]
    fn keeps_hunk_mode_on_the_next_hunk_after_staging() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let baseline = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        fs::write(
            root.join("tracked.txt"),
            format!("{}\n", baseline.join("\n")),
        )
        .unwrap();
        run_git(root, &["add", "tracked.txt"]);
        run_git(root, &["commit", "-m", "expand fixture"]);

        let mut edited = baseline;
        edited[1] = "changed first".to_owned();
        edited[18] = "changed second".to_owned();
        fs::write(root.join("tracked.txt"), format!("{}\n", edited.join("\n"))).unwrap();
        let mut app = App::new(root.to_path_buf());
        for _ in 0..100 {
            let _ = app.poll_worker();
            if app.changes.diff.matches("@@").count() == 2 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.changes.hunk_selection, Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.changes.hunk_selection, Some(0));

        for _ in 0..200 {
            let _ = app.poll_worker();
            let repo = app.repository().unwrap();
            let split_change = repo
                .changes
                .iter()
                .any(|change| change.path == "tracked.txt" && change.staged)
                && repo
                    .changes
                    .iter()
                    .any(|change| change.path == "tracked.txt" && !change.staged);
            if split_change
                && app.changes.hunk_selection == Some(0)
                && app.changes.diff.contains("changed second")
                && !app.changes.diff.contains("changed first")
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(app.changes.hunk_selection, Some(0));
        assert!(app.changes.diff.contains("changed second"));
        assert!(!app.changes.diff.contains("changed first"));
    }

    #[test]
    fn configures_and_requests_an_interactive_editor() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        fs::write(root.join("tracked.txt"), "edited\n").unwrap();
        let settings_path = root.join(".git/hunkle-editor-test-config");
        let mut app = App::new(root.to_path_buf());
        app.settings.editor_command = None;
        app.settings_path = Some(settings_path.clone());

        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        app.editor_input.clear();
        app.handle_paste("code --wait");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let request = app.take_editor_request().unwrap();
        assert_eq!(request.command, ["code", "--wait"]);
        assert_eq!(request.file, root.join("tracked.txt"));
        assert_eq!(request.repository, root);
        assert_eq!(app.settings.editor_command.as_deref(), Some("code --wait"));
        assert!(
            fs::read_to_string(settings_path)
                .unwrap()
                .contains("editor_command=code --wait")
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('E'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.editor_input, "code --wait");
    }

    #[test]
    fn runs_a_custom_git_command_and_keeps_its_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        initialize_repository(root);
        let mut app = App::new(root.to_path_buf());

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Commit);
        assert_eq!(app.view, View::Changes);
        assert_eq!(app.changes.pane, LeftPane::Worktree);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.actions.status, CommandStatus::Input);

        app.handle_paste("rev-parse --abbrev-ref HEAD");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.actions.status, CommandStatus::Running);
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            let _ = app.poll_worker();
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
            let _ = app.poll_worker();
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
            let _ = app.poll_worker();
            if app.changes.diff.contains("first") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.changes.diff.contains("first"));

        fs::write(&tracked, "later\n").unwrap();
        app.session.schedule_status_check_now();
        for _ in 0..100 {
            let _ = app.poll_worker();
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

    fn wait_for_state(app: &mut App, predicate: impl Fn(&App) -> bool) {
        for _ in 0..200 {
            let _ = app.poll_worker();
            if predicate(app) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("application state did not update");
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
