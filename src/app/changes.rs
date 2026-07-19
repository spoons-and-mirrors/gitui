use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use ratatui::widgets::ListState;

use crate::{
    git::{self, Change, RepositoryData},
    tree::{ExplorerRow, WorktreeRow, build_file_tree, build_worktree},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftPane {
    Worktree,
    Files,
}

pub struct ChangesState {
    pub(crate) pane: LeftPane,
    pub(crate) worktree_state: ListState,
    pub(crate) explorer_state: ListState,
    pub(crate) worktree_scroll: usize,
    pub(crate) explorer_scroll: usize,
    pub(crate) history_state: ListState,
    pub(crate) diff: String,
    pub(crate) diff_scroll: u16,
    pub(crate) diff_wrap: bool,
    pub(crate) history_focused: bool,
    pub(crate) collapsed_directories: HashSet<String>,
    pub(crate) collapsed_explorer_directories: HashSet<String>,
    worktree_rows_cache: Vec<WorktreeRow>,
    explorer_rows_cache: Vec<ExplorerRow>,
    preview_generation: u64,
    preview_tx: Sender<PreviewRequest>,
    preview_rx: Receiver<PreviewResult>,
}

struct PreviewRequest {
    generation: u64,
    root: PathBuf,
    task: PreviewTask,
}

enum PreviewTask {
    File(String),
    Commit(String),
    Diff(Change),
}

struct PreviewResult {
    generation: u64,
    root: PathBuf,
    content: String,
}

pub(super) struct ChangesSelection {
    change: Option<(String, bool)>,
    directory: Option<String>,
    explorer_file: Option<String>,
    explorer_directory: Option<String>,
    history_oid: Option<String>,
}

impl ChangesState {
    pub(super) fn new(repo: Option<&RepositoryData>) -> Self {
        let (preview_tx, preview_rx) = preview_worker();
        let collapsed_explorer_directories = default_explorer_collapsed_directories(repo);
        let mut state = Self {
            pane: LeftPane::Worktree,
            worktree_state: ListState::default(),
            explorer_state: ListState::default(),
            worktree_scroll: 0,
            explorer_scroll: 0,
            history_state: ListState::default(),
            diff: String::new(),
            diff_scroll: 0,
            diff_wrap: false,
            history_focused: false,
            collapsed_directories: HashSet::new(),
            collapsed_explorer_directories,
            worktree_rows_cache: Vec::new(),
            explorer_rows_cache: Vec::new(),
            preview_generation: 0,
            preview_tx,
            preview_rx,
        };
        state.rebuild_worktree_rows(repo);
        state.rebuild_explorer_rows(repo);
        state.select_initial_rows(repo);
        state.refresh_diff(repo);
        state
    }

    pub(super) fn reset_repository(&mut self, repo: Option<&RepositoryData>) {
        self.worktree_state = ListState::default();
        self.explorer_state = ListState::default();
        self.worktree_scroll = 0;
        self.explorer_scroll = 0;
        self.history_state = ListState::default();
        self.diff.clear();
        self.diff_scroll = 0;
        self.history_focused = false;
        self.collapsed_directories.clear();
        self.collapsed_explorer_directories = default_explorer_collapsed_directories(repo);
        self.rebuild_worktree_rows(repo);
        self.rebuild_explorer_rows(repo);
        self.select_initial_rows(repo);
        self.refresh_diff(repo);
    }

    pub(super) fn capture_selection(&self, repo: &RepositoryData) -> ChangesSelection {
        ChangesSelection {
            change: self
                .selected_change_index(repo)
                .and_then(|index| repo.changes.get(index))
                .map(|change| (change.path.clone(), change.staged)),
            directory: self.selected_directory_path(repo),
            explorer_file: self.selected_explorer_file_path(repo).map(str::to_owned),
            explorer_directory: self.selected_explorer_directory_path(),
            history_oid: self
                .history_state
                .selected()
                .and_then(|index| repo.history.get(index))
                .map(|commit| commit.oid.clone()),
        }
    }

    pub(super) fn restore_selection(&mut self, repo: &RepositoryData, selection: ChangesSelection) {
        self.rebuild_worktree_rows(Some(repo));
        self.rebuild_explorer_rows(Some(repo));

        let change_index = selection.change.and_then(|(path, staged)| {
            repo.changes
                .iter()
                .position(|change| change.path == path && change.staged == staged)
                .or_else(|| repo.changes.iter().position(|change| change.path == path))
        });
        let change_row = change_index
            .and_then(|index| self.row_for_change(repo, index))
            .or_else(|| {
                let directory = selection.directory.as_ref()?;
                self.worktree_rows(repo)
                    .iter()
                    .position(|row| row.directory_path.as_ref() == Some(directory))
            })
            .or_else(|| self.first_change_row(repo));
        self.worktree_state.select(change_row);

        let history_index = selection
            .history_oid
            .and_then(|oid| repo.history.iter().position(|commit| commit.oid == oid));
        self.history_state.select(history_index);

        let explorer_row = selection
            .explorer_file
            .and_then(|path| {
                let file_index = repo.files.iter().position(|candidate| candidate == &path)?;
                self.row_for_explorer_file(file_index)
            })
            .or_else(|| {
                let directory = selection.explorer_directory.as_ref()?;
                self.explorer_rows()
                    .iter()
                    .position(|row| row.directory_path.as_ref() == Some(directory))
            })
            .or_else(|| self.initial_explorer_row());
        self.explorer_state.select(explorer_row);
        self.refresh_diff(Some(repo));
    }

    pub(crate) fn worktree_rows(&self, _repo: &RepositoryData) -> &[WorktreeRow] {
        &self.worktree_rows_cache
    }

    pub(crate) fn explorer_rows(&self) -> &[ExplorerRow] {
        &self.explorer_rows_cache
    }

    pub(crate) fn selected_explorer_file_path<'a>(
        &self,
        repo: &'a RepositoryData,
    ) -> Option<&'a str> {
        let selected = self.explorer_state.selected()?;
        let file_index = self.explorer_rows().get(selected)?.file_index?;
        repo.files.get(file_index).map(String::as_str)
    }

    pub(super) fn selected_change_index(&self, repo: &RepositoryData) -> Option<usize> {
        let selected = self.worktree_state.selected()?;
        self.worktree_rows(repo).get(selected)?.change_index
    }

    pub(super) fn selected_directory_path(&self, repo: &RepositoryData) -> Option<String> {
        let selected = self.worktree_state.selected()?;
        self.worktree_rows(repo)
            .get(selected)?
            .directory_path
            .clone()
    }

    pub(super) fn selected_explorer_directory_path(&self) -> Option<String> {
        let selected = self.explorer_state.selected()?;
        self.explorer_rows().get(selected)?.directory_path.clone()
    }

    pub(super) fn set_pane(&mut self, pane: LeftPane, repo: Option<&RepositoryData>) -> bool {
        if self.pane == pane {
            return false;
        }
        self.pane = pane;
        self.clear_history_selection();
        if pane == LeftPane::Files && self.explorer_state.selected().is_none() {
            self.explorer_state.select(self.initial_explorer_row());
        }
        self.refresh_diff(repo);
        true
    }

    pub(super) fn select_worktree_row(&mut self, repo: &RepositoryData, index: usize) -> bool {
        if index >= self.worktree_rows(repo).len() {
            return false;
        }
        self.worktree_state.select(Some(index));
        self.clear_history_selection();
        self.refresh_diff(Some(repo));
        true
    }

    pub(super) fn select_explorer_row(&mut self, repo: &RepositoryData, index: usize) -> bool {
        if index >= self.explorer_rows().len() {
            return false;
        }
        self.explorer_state.select(Some(index));
        self.refresh_diff(Some(repo));
        true
    }

    pub(super) fn select_history_row(
        &mut self,
        repo: &RepositoryData,
        relative_row: usize,
    ) -> bool {
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
        self.refresh_diff(Some(repo));
        true
    }

    pub(super) fn move_selection(
        &mut self,
        repo: Option<&RepositoryData>,
        delta: isize,
        worktree_viewport: usize,
        explorer_viewport: usize,
    ) {
        let Some(repo) = repo else {
            return;
        };
        if self.pane == LeftPane::Files {
            move_list(
                &mut self.explorer_state,
                self.explorer_rows_cache.len(),
                delta,
            );
            ensure_selection_visible(
                &mut self.explorer_scroll,
                self.explorer_state.selected(),
                explorer_viewport,
            );
        } else if self.history_focused {
            move_list(&mut self.history_state, repo.history.len(), delta);
        } else {
            let len = self.worktree_rows(repo).len();
            move_list(&mut self.worktree_state, len, delta);
            ensure_selection_visible(
                &mut self.worktree_scroll,
                self.worktree_state.selected(),
                worktree_viewport,
            );
        }
        self.refresh_diff(Some(repo));
    }

    pub(super) fn move_history_selection(&mut self, repo: &RepositoryData, delta: isize) {
        self.history_focused = true;
        move_list(&mut self.history_state, repo.history.len(), delta);
        self.refresh_diff(Some(repo));
    }

    pub(super) fn select_first(
        &mut self,
        repo: Option<&RepositoryData>,
        worktree_viewport: usize,
        explorer_viewport: usize,
    ) {
        let Some(repo) = repo else {
            return;
        };
        if self.pane == LeftPane::Files {
            self.explorer_state
                .select((!self.explorer_rows().is_empty()).then_some(0));
            ensure_selection_visible(
                &mut self.explorer_scroll,
                self.explorer_state.selected(),
                explorer_viewport,
            );
        } else if self.history_focused {
            self.history_state
                .select((!repo.history.is_empty()).then_some(0));
        } else {
            self.worktree_state.select(self.first_change_row(repo));
            ensure_selection_visible(
                &mut self.worktree_scroll,
                self.worktree_state.selected(),
                worktree_viewport,
            );
        }
        self.refresh_diff(Some(repo));
    }

    pub(super) fn select_last(
        &mut self,
        repo: Option<&RepositoryData>,
        worktree_viewport: usize,
        explorer_viewport: usize,
    ) {
        let Some(repo) = repo else {
            return;
        };
        if self.pane == LeftPane::Files {
            self.explorer_state
                .select(self.explorer_rows().len().checked_sub(1));
            ensure_selection_visible(
                &mut self.explorer_scroll,
                self.explorer_state.selected(),
                explorer_viewport,
            );
        } else if self.history_focused {
            self.history_state.select(repo.history.len().checked_sub(1));
        } else {
            self.worktree_state.select(self.last_change_row(repo));
            ensure_selection_visible(
                &mut self.worktree_scroll,
                self.worktree_state.selected(),
                worktree_viewport,
            );
        }
        self.refresh_diff(Some(repo));
    }

    pub(super) fn scroll_worktree(
        &mut self,
        repo: Option<&RepositoryData>,
        viewport: usize,
        delta: isize,
    ) {
        let len = repo.map_or(0, |repo| self.worktree_rows(repo).len());
        scroll_viewport(&mut self.worktree_scroll, len, viewport, delta);
    }

    pub(super) fn scroll_explorer(&mut self, viewport: usize, delta: isize) {
        scroll_viewport(
            &mut self.explorer_scroll,
            self.explorer_rows_cache.len(),
            viewport,
            delta,
        );
    }

    pub(super) fn scroll_diff_by(&mut self, maximum: u16, delta: isize) {
        self.diff_scroll = if delta > 0 {
            self.diff_scroll.saturating_add(delta as u16).min(maximum)
        } else {
            self.diff_scroll.saturating_sub(delta.unsigned_abs() as u16)
        };
    }

    pub(super) fn set_diff_scroll_from_track(
        &mut self,
        row: u16,
        track_y: u16,
        track_height: u16,
        thumb_height: u16,
        drag_offset: u16,
        maximum: u16,
    ) {
        let travel = track_height.saturating_sub(thumb_height);
        if travel == 0 || maximum == 0 {
            self.diff_scroll = 0;
            return;
        }
        let position = row
            .saturating_sub(track_y)
            .saturating_sub(drag_offset)
            .min(travel);
        self.diff_scroll = ((u32::from(position) * u32::from(maximum) + u32::from(travel) / 2)
            / u32::from(travel)) as u16;
    }

    pub(super) fn toggle_wrap(&mut self) -> bool {
        self.diff_wrap = !self.diff_wrap;
        self.diff_wrap
    }

    pub(super) fn clear_history_selection(&mut self) {
        self.history_focused = false;
        self.history_state.select(None);
    }

    pub(super) fn toggle_selected_explorer_directory(&mut self, repo: Option<&RepositoryData>) {
        let Some(path) = self.selected_explorer_directory_path() else {
            return;
        };
        if !self.collapsed_explorer_directories.remove(&path) {
            self.collapsed_explorer_directories.insert(path.clone());
        }
        self.rebuild_explorer_rows(repo);
        self.select_explorer_directory(&path);
        self.refresh_diff(repo);
    }

    pub(super) fn expand_or_descend_explorer(&mut self, repo: Option<&RepositoryData>) {
        let Some(index) = self.explorer_state.selected() else {
            return;
        };
        let Some(row) = self.explorer_rows_cache.get(index) else {
            return;
        };
        let Some(path) = row.directory_path.clone() else {
            return;
        };
        let expanded = row.directory_expanded;
        let depth = row.depth;
        if expanded == Some(false) {
            self.collapsed_explorer_directories.remove(&path);
            self.rebuild_explorer_rows(repo);
            self.select_explorer_directory(&path);
        } else if self
            .explorer_rows_cache
            .get(index + 1)
            .is_some_and(|child| child.depth > depth)
        {
            self.explorer_state.select(Some(index + 1));
        }
        self.refresh_diff(repo);
    }

    pub(super) fn collapse_or_ascend_explorer(&mut self, repo: Option<&RepositoryData>) {
        let Some(index) = self.explorer_state.selected() else {
            return;
        };
        let Some(row) = self.explorer_rows_cache.get(index) else {
            return;
        };
        let row_depth = row.depth;
        let directory = row.directory_path.clone();
        if row.directory_expanded == Some(true)
            && let Some(path) = directory
        {
            self.collapsed_explorer_directories.insert(path.clone());
            self.rebuild_explorer_rows(repo);
            self.select_explorer_directory(&path);
            self.refresh_diff(repo);
            return;
        }
        if let Some(parent) = self.explorer_rows_cache[..index]
            .iter()
            .rposition(|candidate| candidate.depth < row_depth)
        {
            self.explorer_state.select(Some(parent));
            self.refresh_diff(repo);
        }
    }

    pub(super) fn toggle_selected_directory(&mut self, repo: Option<&RepositoryData>) {
        let Some(repo) = repo else {
            return;
        };
        let Some(path) = self.selected_directory_path(repo) else {
            return;
        };
        if !self.collapsed_directories.remove(&path) {
            self.collapsed_directories.insert(path.clone());
        }
        self.rebuild_worktree_rows(Some(repo));
        self.select_directory(repo, &path);
        self.refresh_diff(Some(repo));
    }

    pub(super) fn expand_or_descend_worktree(&mut self, repo: Option<&RepositoryData>) {
        let Some(repo) = repo else {
            return;
        };
        let Some(index) = self.worktree_state.selected() else {
            return;
        };
        let Some(row) = self.worktree_rows(repo).get(index) else {
            return;
        };
        let Some(path) = row.directory_path.clone() else {
            return;
        };
        let expanded = row.directory_expanded;
        let descend = self
            .worktree_rows(repo)
            .get(index + 1)
            .is_some_and(|child| child.depth > row.depth);
        if expanded == Some(false) {
            self.collapsed_directories.remove(&path);
            self.rebuild_worktree_rows(Some(repo));
            self.select_directory(repo, &path);
        } else if descend {
            self.worktree_state.select(Some(index + 1));
        }
        self.refresh_diff(Some(repo));
    }

    pub(super) fn collapse_or_ascend_worktree(&mut self, repo: Option<&RepositoryData>) {
        let Some(repo) = repo else {
            return;
        };
        let Some(index) = self.worktree_state.selected() else {
            return;
        };
        let Some(row) = self.worktree_rows(repo).get(index) else {
            return;
        };
        let row_depth = row.depth;
        let directory = row.directory_path.clone();
        if let Some(path) = directory
            && row.directory_expanded == Some(true)
        {
            self.collapsed_directories.insert(path.clone());
            self.rebuild_worktree_rows(Some(repo));
            self.select_directory(repo, &path);
            self.refresh_diff(Some(repo));
            return;
        }
        if let Some(parent) = self.worktree_rows(repo)[..index]
            .iter()
            .rposition(|candidate| candidate.depth < row_depth)
        {
            self.worktree_state.select(Some(parent));
            self.refresh_diff(Some(repo));
        }
    }

    pub(super) fn refresh_diff(&mut self, repo: Option<&RepositoryData>) {
        self.diff_scroll = 0;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let Some(repo) = repo else {
            self.diff.clear();
            return;
        };
        if self.pane == LeftPane::Files {
            let Some(row) = self
                .explorer_state
                .selected()
                .and_then(|index| self.explorer_rows_cache.get(index))
            else {
                self.diff = "Select a file to preview".to_owned();
                return;
            };
            if let Some(index) = row.file_index {
                self.request_preview(repo, PreviewTask::File(repo.files[index].clone()));
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
            self.request_preview(repo, PreviewTask::Commit(commit.oid.clone()));
            return;
        }
        let rows = self.worktree_rows(repo);
        let Some(row) = self
            .worktree_state
            .selected()
            .and_then(|index| rows.get(index))
        else {
            self.diff = "Working tree clean".to_owned();
            return;
        };
        if let Some(index) = row.change_index {
            self.request_preview(repo, PreviewTask::Diff(repo.changes[index].clone()));
        } else if let Some(path) = &row.directory_path {
            let count = repo
                .changes
                .iter()
                .filter(|change| change.path.starts_with(&format!("{path}/")))
                .count();
            self.diff = format!("{count} changed files in {path}/");
        }
    }

    pub(super) fn poll_preview(&mut self, active_root: Option<&Path>) -> bool {
        let mut changed = false;
        while let Ok(result) = self.preview_rx.try_recv() {
            if result.generation == self.preview_generation
                && active_root.is_some_and(|root| root == result.root)
            {
                self.diff = result.content;
                changed = true;
            }
        }
        changed
    }

    fn request_preview(&mut self, repo: &RepositoryData, task: PreviewTask) {
        self.diff = "Loading preview…".to_owned();
        let _ = self.preview_tx.send(PreviewRequest {
            generation: self.preview_generation,
            root: repo.root.clone(),
            task,
        });
    }

    fn rebuild_explorer_rows(&mut self, repo: Option<&RepositoryData>) {
        self.explorer_rows_cache = repo.map_or_else(Vec::new, |repo| {
            build_file_tree(&repo.files, &self.collapsed_explorer_directories)
        });
    }

    fn rebuild_worktree_rows(&mut self, repo: Option<&RepositoryData>) {
        self.worktree_rows_cache = repo.map_or_else(Vec::new, |repo| {
            build_worktree(&repo.changes, &self.collapsed_directories)
        });
    }

    fn select_initial_rows(&mut self, repo: Option<&RepositoryData>) {
        self.worktree_state
            .select(repo.and_then(|repo| self.first_change_row(repo)));
        self.history_state.select(None);
        self.explorer_state.select(self.initial_explorer_row());
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

    fn initial_explorer_row(&self) -> Option<usize> {
        self.first_explorer_file_row()
            .or_else(|| (!self.explorer_rows().is_empty()).then_some(0))
    }

    fn select_directory(&mut self, repo: &RepositoryData, path: &str) {
        let row = self
            .worktree_rows(repo)
            .iter()
            .position(|row| row.directory_path.as_deref() == Some(path));
        self.worktree_state.select(row);
    }

    fn row_for_change(&self, repo: &RepositoryData, change_index: usize) -> Option<usize> {
        self.worktree_rows(repo)
            .iter()
            .position(|row| row.change_index == Some(change_index))
    }

    fn first_change_row(&self, repo: &RepositoryData) -> Option<usize> {
        self.worktree_rows(repo)
            .iter()
            .position(|row| row.change_index.is_some())
    }

    fn last_change_row(&self, repo: &RepositoryData) -> Option<usize> {
        self.worktree_rows(repo)
            .iter()
            .rposition(|row| row.change_index.is_some())
    }
}

fn preview_worker() -> (Sender<PreviewRequest>, Receiver<PreviewResult>) {
    let (request_tx, request_rx) = mpsc::channel::<PreviewRequest>();
    let (result_tx, result_rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(latest) = request_rx.try_recv() {
                request = latest;
            }
            let content = match &request.task {
                PreviewTask::File(path) => git::file_content(&request.root, path),
                PreviewTask::Commit(oid) => git::commit_diff(&request.root, oid),
                PreviewTask::Diff(change) => git::diff(&request.root, change),
            }
            .unwrap_or_else(|error| error.to_string());
            if result_tx
                .send(PreviewResult {
                    generation: request.generation,
                    root: request.root,
                    content,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (request_tx, result_rx)
}

fn default_explorer_collapsed_directories(repo: Option<&RepositoryData>) -> HashSet<String> {
    repo.map_or_else(HashSet::new, |repo| {
        build_file_tree(&repo.files, &HashSet::new())
            .into_iter()
            .filter_map(|row| row.directory_path)
            .collect()
    })
}

fn move_list(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0);
    let next = (current as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
    state.select(Some(next));
}

fn scroll_viewport(scroll: &mut usize, len: usize, viewport: usize, delta: isize) {
    let maximum = len.saturating_sub(viewport);
    *scroll = if delta > 0 {
        scroll.saturating_add(delta as usize).min(maximum)
    } else {
        scroll.saturating_sub(delta.unsigned_abs())
    };
}

fn ensure_selection_visible(scroll: &mut usize, selected: Option<usize>, viewport: usize) {
    let Some(selected) = selected else { return };
    if viewport == 0 {
        return;
    }
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= scroll.saturating_add(viewport) {
        *scroll = selected.saturating_add(1).saturating_sub(viewport);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::git::Change;

    use super::*;

    #[test]
    fn starts_files_collapsed_but_keeps_worktree_expanded() {
        let repo = RepositoryData {
            root: PathBuf::new(),
            branch: "main".to_owned(),
            changes: vec![Change {
                path: "src/main.rs".to_owned(),
                original_path: None,
                code: 'M',
                staged: false,
                additions: 0,
                deletions: 0,
            }],
            files: vec![
                "src/app/mod.rs".to_owned(),
                "src/main.rs".to_owned(),
                "README.md".to_owned(),
            ],
            history: Vec::new(),
            commits: Vec::new(),
        };

        let mut state = ChangesState::new(Some(&repo));
        assert!(state.collapsed_directories.is_empty());
        assert_eq!(
            state.collapsed_explorer_directories,
            HashSet::from(["src".to_owned(), "src/app".to_owned()])
        );
        assert_eq!(
            state
                .explorer_rows()
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["src", "README.md"]
        );
        assert_eq!(state.explorer_state.selected(), Some(1));

        state.explorer_state.select(Some(0));
        state.expand_or_descend_explorer(Some(&repo));
        assert_eq!(
            state
                .explorer_rows()
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["src", "app", "main.rs", "README.md"]
        );
        assert_eq!(state.explorer_rows()[1].directory_expanded, Some(false));
    }
}
