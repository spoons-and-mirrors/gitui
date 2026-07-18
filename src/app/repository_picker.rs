use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

#[derive(Debug, Clone)]
pub struct PickerEntry {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) action: PickerAction,
    pub(crate) is_repo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    Open,
    Navigate,
}

#[derive(Debug)]
pub struct RepositoryPicker {
    pub(crate) directory: PathBuf,
    pub(crate) path_input: String,
    pub(crate) editing_path: bool,
    pub(crate) entries: Vec<PickerEntry>,
    pub(crate) state: ListState,
    pub(crate) matches: Vec<PickerEntry>,
    pub(crate) match_state: ListState,
    pub(crate) searching: bool,
    pub(crate) error: Option<String>,
    directory_index: Vec<PathBuf>,
    index_rx: Option<Receiver<Vec<PathBuf>>>,
}

pub(super) enum PickerCommand {
    None,
    Close,
    Quit,
    Open(PathBuf),
}

impl RepositoryPicker {
    pub(super) fn new(directory: PathBuf) -> Self {
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

    pub(super) fn handle_key(&mut self, key: KeyEvent, can_close: bool) -> PickerCommand {
        if self.editing_path {
            match key.code {
                KeyCode::Esc => {
                    self.editing_path = false;
                    self.matches.clear();
                }
                KeyCode::Enter => return self.confirm_path(),
                KeyCode::Tab => self.accept_completion(),
                KeyCode::Down => self.move_match_selection(1),
                KeyCode::Up => self.move_match_selection(-1),
                KeyCode::Backspace => {
                    self.path_input.pop();
                    self.refresh_matches();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.path_input.clear();
                    self.refresh_matches();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.path_input.push(character);
                    self.refresh_matches();
                }
                _ => {}
            }
            return PickerCommand::None;
        }
        match key.code {
            KeyCode::Esc if can_close => PickerCommand::Close,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                PickerCommand::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                PickerCommand::None
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                self.go_parent();
                PickerCommand::None
            }
            KeyCode::Enter => self.activate_selected(true),
            KeyCode::Right | KeyCode::Char('l') => self.activate_selected(false),
            KeyCode::Char('p') => {
                self.begin_search(Some(""));
                PickerCommand::None
            }
            KeyCode::Char('/') => {
                self.begin_search(Some(std::path::MAIN_SEPARATOR_STR));
                PickerCommand::None
            }
            KeyCode::Char('r') => {
                self.reload();
                PickerCommand::None
            }
            KeyCode::Char('q') if !can_close => PickerCommand::Quit,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.begin_search(Some(&character.to_string()));
                PickerCommand::None
            }
            _ => PickerCommand::None,
        }
    }

    pub(super) fn paste(&mut self, text: &str) {
        if self.editing_path {
            self.path_input.push_str(text);
            self.refresh_matches();
        }
    }

    pub(super) fn activate_selected(&mut self, open_repositories: bool) -> PickerCommand {
        let Some(entry) = self.selected().cloned() else {
            return PickerCommand::None;
        };
        if open_repositories && entry.action == PickerAction::Navigate && entry.is_repo {
            return PickerCommand::Open(entry.path);
        }
        match entry.action {
            PickerAction::Navigate => {
                self.navigate(entry.path);
                PickerCommand::None
            }
            PickerAction::Open => PickerCommand::Open(entry.path),
        }
    }

    pub(super) fn confirm_path(&mut self) -> PickerCommand {
        let path = self.selected_match_path();
        if !path.is_dir() {
            self.error = Some(format!("Directory not found: {}", path.display()));
            return PickerCommand::None;
        }
        if is_repository_directory(&path) {
            PickerCommand::Open(path)
        } else {
            self.navigate(path);
            self.editing_path = false;
            self.matches.clear();
            PickerCommand::None
        }
    }

    pub(super) fn reload(&mut self) {
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

    pub(super) fn move_selection(&mut self, delta: isize) {
        move_list(&mut self.state, self.entries.len(), delta);
    }

    pub(super) fn begin_search(&mut self, initial: Option<&str>) {
        self.editing_path = true;
        self.error = None;
        if let Some(initial) = initial {
            self.path_input = initial.to_owned();
        }
        self.refresh_matches();
    }

    pub(super) fn poll_index(&mut self) {
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

    pub(super) fn navigate(&mut self, path: PathBuf) {
        self.directory = path;
        self.path_input = self.directory.display().to_string();
        self.reload();
    }

    fn selected(&self) -> Option<&PickerEntry> {
        self.state
            .selected()
            .and_then(|index| self.entries.get(index))
    }

    fn move_match_selection(&mut self, delta: isize) {
        move_list(&mut self.match_state, self.matches.len(), delta);
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

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
