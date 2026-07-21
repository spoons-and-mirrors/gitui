use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use super::TextInput;

pub(crate) const DEFAULT_WIDTH: u16 = 26;
pub(crate) const MINIMUM_WIDTH: u16 = 18;

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspacePanelPlacement {
    Off,
    Left,
    Right,
}

impl AgentStatus {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("idle") => Self::Idle,
            Some("working") => Self::Working,
            Some("blocked") => Self::Blocked,
            Some("done") => Self::Done,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HerdrWorkspace {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) branch: Option<String>,
    pub(crate) parent_workspace_id: Option<String>,
    pub(crate) pane_count: usize,
    pub(crate) focused: bool,
    pub(crate) status: AgentStatus,
    repo_key: Option<String>,
    repo_root: Option<PathBuf>,
    linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HerdrAgent {
    pub(crate) name: String,
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) focused: bool,
    pub(crate) status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceGroup {
    pub(crate) name: String,
    pub(crate) expanded: bool,
    workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceDeleteKind {
    Workspace {
        pane_count: usize,
    },
    Worktree {
        path: Option<PathBuf>,
        parent_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceDeleteDialog {
    pub(crate) workspace_id: String,
    pub(crate) label: String,
    pub(crate) kind: WorkspaceDeleteKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspacePanelRow {
    Header,
    Group(usize),
    Workspace(usize),
    Spacer,
    AgentHeader,
    Agent(usize),
    EmptyAgents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceDropTarget {
    Group(usize),
    Ungrouped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceDrag {
    workspace: usize,
    active: bool,
    target: Option<WorkspaceDropTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectionKey {
    Workspace(String),
    Agent(String),
}

enum Completion {
    Snapshot(Result<(Vec<HerdrWorkspace>, Vec<HerdrAgent>), String>),
    WorkspaceFocus {
        request_id: u64,
        result: Result<(), String>,
    },
    Action {
        result: Result<(), String>,
        reopen_path: Option<PathBuf>,
    },
}

struct PendingWorkspaceFocus {
    request_id: u64,
    workspace_id: String,
}

pub(crate) struct WorkspacePanel {
    enabled: bool,
    layout_available: bool,
    pub(crate) placement: WorkspacePanelPlacement,
    pub(crate) workspaces: Vec<HerdrWorkspace>,
    pub(crate) agents: Vec<HerdrAgent>,
    pub(crate) groups: Vec<WorkspaceGroup>,
    pub(crate) selected: Option<usize>,
    pub(crate) scroll: usize,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) group_input: TextInput,
    pub(crate) group_editing: bool,
    pub(crate) group_error: Option<String>,
    pub(crate) create_menu_open: bool,
    pub(crate) create_menu_choice: usize,
    pub(crate) delete_dialog: Option<WorkspaceDeleteDialog>,
    groups_path: Option<PathBuf>,
    workspace_drag: Option<WorkspaceDrag>,
    last_click: Option<(SelectionKey, Instant)>,
    host_workspace_id: Option<String>,
    pending_workspace_focus: Option<PendingWorkspaceFocus>,
    next_focus_request_id: u64,
    sender: Sender<Completion>,
    receiver: Receiver<Completion>,
    next_refresh: Instant,
}

impl WorkspacePanel {
    pub(crate) fn detect(groups_path: Option<PathBuf>) -> Self {
        #[cfg(test)]
        let enabled = false;
        #[cfg(not(test))]
        let enabled = std::env::var("HERDR_ENV").ok().as_deref() == Some("1");
        let mut panel = Self::new(enabled, groups_path);
        if enabled {
            panel.host_workspace_id = std::env::var("HERDR_WORKSPACE_ID").ok();
        }
        panel
    }

    fn new(enabled: bool, groups_path: Option<PathBuf>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut groups = if enabled {
            groups_path.as_deref().map(load_groups).unwrap_or_default()
        } else {
            Vec::new()
        };
        sort_groups(&mut groups);
        Self {
            enabled,
            layout_available: false,
            placement: if enabled {
                WorkspacePanelPlacement::Left
            } else {
                WorkspacePanelPlacement::Off
            },
            workspaces: Vec::new(),
            agents: Vec::new(),
            groups,
            selected: None,
            scroll: 0,
            loading: false,
            error: None,
            group_input: TextInput::default(),
            group_editing: false,
            group_error: None,
            create_menu_open: false,
            create_menu_choice: 0,
            delete_dialog: None,
            groups_path,
            workspace_drag: None,
            last_click: None,
            host_workspace_id: None,
            pending_workspace_focus: None,
            next_focus_request_id: 0,
            sender,
            receiver,
            next_refresh: Instant::now(),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn is_available(&self) -> bool {
        self.enabled && self.layout_available
    }

    pub(crate) fn set_layout_available(&mut self, available: bool) {
        self.layout_available = available;
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.placement != WorkspacePanelPlacement::Off
    }

    pub(crate) fn show_left(&mut self) {
        self.placement = WorkspacePanelPlacement::Left;
    }

    pub(crate) fn hide(&mut self) {
        self.placement = WorkspacePanelPlacement::Off;
    }

    pub(crate) fn cycle_placement(&mut self) {
        self.placement = match self.placement {
            WorkspacePanelPlacement::Off => WorkspacePanelPlacement::Left,
            WorkspacePanelPlacement::Left => WorkspacePanelPlacement::Right,
            WorkspacePanelPlacement::Right => WorkspacePanelPlacement::Off,
        };
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.workspaces.len().saturating_add(self.agents.len())
    }

    pub(crate) fn rows(&self) -> Vec<WorkspacePanelRow> {
        let mut rows = vec![WorkspacePanelRow::Header];
        for (group_index, group) in self.groups.iter().enumerate() {
            rows.push(WorkspacePanelRow::Group(group_index));
            if group.expanded {
                for (index, workspace) in
                    self.workspaces.iter().enumerate().filter(|(_, workspace)| {
                        !workspace.linked_worktree && group.workspace_ids.contains(&workspace.id)
                    })
                {
                    rows.push(WorkspacePanelRow::Workspace(index));
                    rows.extend(
                        self.child_workspace_indices(&workspace.id)
                            .into_iter()
                            .map(WorkspacePanelRow::Workspace),
                    );
                }
            }
        }
        for (index, workspace) in self.workspaces.iter().enumerate().filter(|(_, workspace)| {
            !workspace.linked_worktree && self.group_for_workspace_id(&workspace.id).is_none()
        }) {
            rows.push(WorkspacePanelRow::Workspace(index));
            rows.extend(
                self.child_workspace_indices(&workspace.id)
                    .into_iter()
                    .map(WorkspacePanelRow::Workspace),
            );
        }
        rows.extend(
            self.workspaces
                .iter()
                .enumerate()
                .filter(|(_, workspace)| {
                    workspace.linked_worktree && workspace.parent_workspace_id.is_none()
                })
                .map(|(index, _)| WorkspacePanelRow::Workspace(index)),
        );
        rows.push(WorkspacePanelRow::Spacer);
        rows.push(WorkspacePanelRow::AgentHeader);
        if self.agents.is_empty() {
            rows.push(WorkspacePanelRow::EmptyAgents);
        } else {
            rows.extend((0..self.agents.len()).map(WorkspacePanelRow::Agent));
        }
        rows
    }

    pub(crate) fn poll(&mut self) -> (bool, Option<String>, Option<PathBuf>) {
        if !self.enabled {
            return (false, None, None);
        }

        let mut changed = false;
        let mut action_error = None;
        let mut reopen_path = None;
        while let Ok(completion) = self.receiver.try_recv() {
            changed = true;
            match completion {
                Completion::Snapshot(result) => {
                    self.loading = false;
                    match result {
                        Ok((workspaces, agents)) => {
                            let previous = self.selection_key();
                            self.workspaces = workspaces;
                            self.agents = agents;
                            self.reconcile_pending_workspace_focus();
                            self.error = None;
                            if self.reconcile_group_workspace_ids()
                                && let Err(error) = self.persist_groups()
                            {
                                action_error = Some(error);
                            }
                            self.restore_selection(previous);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Completion::WorkspaceFocus { request_id, result } => {
                    let is_current = self
                        .pending_workspace_focus
                        .as_ref()
                        .is_some_and(|pending| pending.request_id == request_id);
                    if is_current {
                        match result {
                            Ok(()) => {
                                self.pending_workspace_focus = None;
                                self.select_host_workspace();
                                self.next_refresh = Instant::now();
                            }
                            Err(error) => {
                                self.pending_workspace_focus = None;
                                action_error = Some(error);
                            }
                        }
                    }
                }
                Completion::Action {
                    result,
                    reopen_path: action_reopen_path,
                } => match result {
                    Ok(()) => {
                        self.next_refresh = Instant::now();
                        reopen_path = action_reopen_path;
                    }
                    Err(error) => action_error = Some(error),
                },
            }
        }

        if !self.loading && Instant::now() >= self.next_refresh {
            self.start_snapshot();
            changed = true;
        }
        (changed, action_error, reopen_path)
    }

    pub(crate) fn refresh(&mut self) {
        if self.enabled && !self.loading {
            self.next_refresh = Instant::now();
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> WorkspacePanelEffect {
        if self.delete_dialog.is_some() {
            return self.handle_delete_dialog(key);
        }
        if self.group_editing {
            return self.handle_group_input(key);
        }
        if self.create_menu_open {
            return self.handle_create_menu(key);
        }
        match key.code {
            KeyCode::Esc => WorkspacePanelEffect::Close,
            KeyCode::Char('w') if key.modifiers.is_empty() => WorkspacePanelEffect::Cycle,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                WorkspacePanelEffect::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                WorkspacePanelEffect::None
            }
            KeyCode::Home => {
                self.selected = self.visible_selections().first().copied();
                WorkspacePanelEffect::None
            }
            KeyCode::End => {
                self.selected = self.visible_selections().last().copied();
                WorkspacePanelEffect::None
            }
            KeyCode::Enter => {
                self.focus_selected();
                WorkspacePanelEffect::None
            }
            KeyCode::Char('r') => {
                self.refresh();
                WorkspacePanelEffect::None
            }
            KeyCode::Char('g') if key.modifiers.is_empty() => {
                self.begin_group();
                WorkspacePanelEffect::None
            }
            KeyCode::Delete if key.modifiers.is_empty() => self.begin_delete(),
            _ => WorkspacePanelEffect::Unhandled,
        }
    }

    fn begin_delete(&mut self) -> WorkspacePanelEffect {
        let Some(workspace) = self
            .selected
            .and_then(|selected| self.workspaces.get(selected))
        else {
            return WorkspacePanelEffect::Notice("Select a workspace to close".to_owned());
        };
        let kind = if workspace.linked_worktree {
            let parent_path = workspace
                .parent_workspace_id
                .as_deref()
                .and_then(|parent_id| {
                    self.workspaces
                        .iter()
                        .find(|candidate| candidate.id == parent_id)
                })
                .and_then(|parent| parent.path.clone())
                .or_else(|| workspace.repo_root.clone());
            WorkspaceDeleteKind::Worktree {
                path: workspace.path.clone(),
                parent_path,
            }
        } else {
            WorkspaceDeleteKind::Workspace {
                pane_count: workspace.pane_count,
            }
        };
        let workspace_id = workspace.id.clone();
        let label = workspace.label.clone();
        self.close_create_menu();
        self.delete_dialog = Some(WorkspaceDeleteDialog {
            workspace_id,
            label,
            kind,
        });
        WorkspacePanelEffect::None
    }

    fn handle_delete_dialog(&mut self, key: KeyEvent) -> WorkspacePanelEffect {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.delete_dialog = None;
                WorkspacePanelEffect::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let Some(dialog) = self.delete_dialog.take() else {
                    return WorkspacePanelEffect::None;
                };
                match dialog.kind {
                    WorkspaceDeleteKind::Workspace { .. } => {
                        WorkspacePanelEffect::CloseWorkspace(dialog.workspace_id)
                    }
                    WorkspaceDeleteKind::Worktree { path, parent_path } => {
                        WorkspacePanelEffect::DeleteWorktree {
                            workspace_id: dialog.workspace_id,
                            path,
                            parent_path,
                        }
                    }
                }
            }
            _ => WorkspacePanelEffect::None,
        }
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if self.group_editing {
            self.group_input.insert(text);
            self.group_error = None;
        }
    }

    pub(crate) fn begin_group(&mut self) {
        self.group_input.clear();
        self.group_input.focus();
        self.group_error = None;
        self.group_editing = true;
    }

    pub(crate) fn create_workspace(&self, path: Option<&Path>) {
        self.start_action(workspace_create_args(path));
    }

    pub(crate) fn create_worktree(&self, workspace_id: &str) {
        self.start_action(worktree_create_args(workspace_id));
    }

    pub(crate) fn close_workspace(&self, workspace_id: &str) {
        self.start_action(workspace_close_args(workspace_id));
    }

    pub(crate) fn delete_worktree(&self, workspace_id: &str, reopen_path: Option<PathBuf>) {
        self.start_action_with_reopen(worktree_remove_args(workspace_id), reopen_path);
    }

    pub(crate) fn toggle_create_menu(&mut self) {
        self.create_menu_open = !self.create_menu_open;
        self.create_menu_choice = 0;
    }

    pub(crate) fn close_create_menu(&mut self) {
        self.create_menu_open = false;
        self.create_menu_choice = 0;
    }

    pub(crate) fn selected_workspace_id(&self) -> Option<&str> {
        self.selected
            .and_then(|selected| self.workspaces.get(selected))
            .map(|workspace| workspace.id.as_str())
    }

    pub(crate) fn activate_create_choice(&mut self, choice: usize) -> WorkspacePanelEffect {
        let effect = match choice {
            0 => WorkspacePanelEffect::CreateWorkspace,
            1 => self
                .selected_workspace_id()
                .map(|id| WorkspacePanelEffect::CreateWorktree(id.to_owned()))
                .unwrap_or(WorkspacePanelEffect::None),
            _ => WorkspacePanelEffect::None,
        };
        if effect != WorkspacePanelEffect::None {
            self.close_create_menu();
        }
        effect
    }

    fn handle_create_menu(&mut self, key: KeyEvent) -> WorkspacePanelEffect {
        match key.code {
            KeyCode::Esc => {
                self.close_create_menu();
                WorkspacePanelEffect::None
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                if self.selected_workspace_id().is_some() {
                    self.create_menu_choice = 1;
                }
                WorkspacePanelEffect::None
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.create_menu_choice = 0;
                WorkspacePanelEffect::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_create_choice(self.create_menu_choice)
            }
            _ => WorkspacePanelEffect::None,
        }
    }

    fn handle_group_input(&mut self, key: KeyEvent) -> WorkspacePanelEffect {
        self.group_input.focus();
        match key.code {
            KeyCode::Esc => {
                self.group_editing = false;
                self.group_error = None;
            }
            KeyCode::Enter => return self.submit_group(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.group_input.select_all();
            }
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.group_input.delete_word();
                self.group_error = None;
            }
            KeyCode::Left => self.group_input.move_left(),
            KeyCode::Right => self.group_input.move_right(),
            KeyCode::Home => self.group_input.move_home(),
            KeyCode::End => self.group_input.move_end(),
            KeyCode::Delete => self.group_input.delete(),
            KeyCode::Backspace => self.group_input.backspace(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.group_input.insert_char(character);
                self.group_error = None;
            }
            _ => {}
        }
        WorkspacePanelEffect::None
    }

    fn submit_group(&mut self) -> WorkspacePanelEffect {
        let name = self.group_input.text().trim();
        if name.is_empty() {
            self.group_error = Some("Group name is required".to_owned());
            return WorkspacePanelEffect::None;
        }
        if self
            .groups
            .iter()
            .any(|group| group.name.eq_ignore_ascii_case(name))
        {
            self.group_error = Some("Group already exists".to_owned());
            return WorkspacePanelEffect::None;
        }
        self.groups.push(WorkspaceGroup {
            name: name.to_owned(),
            expanded: true,
            workspace_ids: Vec::new(),
        });
        sort_groups(&mut self.groups);
        self.group_editing = false;
        self.group_error = None;
        match self.persist_groups() {
            Ok(()) => WorkspacePanelEffect::None,
            Err(error) => WorkspacePanelEffect::Notice(error),
        }
    }

    pub(crate) fn select_workspace(&mut self, index: usize) -> bool {
        if index >= self.workspaces.len() {
            return false;
        }
        self.selected = Some(index);
        true
    }

    pub(crate) fn select_agent(&mut self, index: usize) -> bool {
        if index >= self.agents.len() {
            return false;
        }
        self.selected = Some(self.workspaces.len().saturating_add(index));
        true
    }

    pub(crate) fn click_workspace(&mut self, index: usize) -> WorkspacePanelEffect {
        if !self.select_workspace(index) {
            return WorkspacePanelEffect::None;
        }
        let key = SelectionKey::Workspace(self.workspaces[index].id.clone());
        if self.is_double_click(&key) {
            self.focus_selected();
            self.last_click = Some((key, Instant::now()));
            return WorkspacePanelEffect::None;
        }
        self.last_click = Some((key, Instant::now()));
        self.workspaces[index].path.clone().map_or_else(
            || WorkspacePanelEffect::Notice("Workspace has no directory to open".to_owned()),
            WorkspacePanelEffect::OpenWorkspace,
        )
    }

    pub(crate) fn click_agent(&mut self, index: usize) {
        if !self.select_agent(index) {
            return;
        }
        let key = SelectionKey::Agent(self.agents[index].pane_id.clone());
        if self.is_double_click(&key) {
            self.focus_selected();
        }
        self.last_click = Some((key, Instant::now()));
    }

    fn is_double_click(&self, key: &SelectionKey) -> bool {
        self.last_click
            .as_ref()
            .is_some_and(|(previous, at)| previous == key && at.elapsed() <= DOUBLE_CLICK_INTERVAL)
    }

    pub(crate) fn toggle_group(&mut self, index: usize) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.expanded = !group.expanded;
        self.ensure_visible_selection();
        let _ = self.persist_groups();
    }

    pub(crate) fn group_for_workspace(&self, index: usize) -> Option<usize> {
        let workspace = self.workspaces.get(index)?;
        let workspace_id = workspace
            .parent_workspace_id
            .as_deref()
            .unwrap_or(&workspace.id);
        self.group_for_workspace_id(workspace_id)
    }

    pub(crate) fn workspace_indent(&self, index: usize) -> &'static str {
        let Some(workspace) = self.workspaces.get(index) else {
            return "";
        };
        match (
            self.group_for_workspace(index).is_some(),
            workspace.linked_worktree,
        ) {
            (true, true) => "  ",
            (true, false) | (false, true) => " ",
            (false, false) => "",
        }
    }

    pub(crate) fn workspace_is_active(&self, index: usize) -> bool {
        let Some(workspace) = self.workspaces.get(index) else {
            return false;
        };
        if let Some(pending) = self.pending_workspace_focus.as_ref() {
            return workspace.id == pending.workspace_id;
        }
        self.host_workspace_id
            .as_ref()
            .map_or(workspace.focused, |host| workspace.id == *host)
    }

    fn group_for_workspace_id(&self, id: &str) -> Option<usize> {
        self.groups.iter().position(|group| {
            group
                .workspace_ids
                .iter()
                .any(|workspace_id| workspace_id == id)
        })
    }

    pub(crate) fn begin_workspace_drag(&mut self, workspace: usize) -> bool {
        if self
            .workspaces
            .get(workspace)
            .is_none_or(|workspace| workspace.linked_worktree)
        {
            return false;
        }
        self.workspace_drag = Some(WorkspaceDrag {
            workspace,
            active: false,
            target: None,
        });
        true
    }

    pub(crate) fn update_workspace_drag(&mut self, target: Option<WorkspaceDropTarget>) {
        if let Some(drag) = self.workspace_drag.as_mut() {
            drag.active = true;
            drag.target = target;
        }
    }

    pub(crate) fn finish_workspace_drag(&mut self) -> WorkspacePanelEffect {
        let Some(drag) = self.workspace_drag.take() else {
            return WorkspacePanelEffect::None;
        };
        if !drag.active {
            return self.click_workspace(drag.workspace);
        }
        let Some(target) = drag.target else {
            return WorkspacePanelEffect::None;
        };
        let Some(workspace_id) = self
            .workspaces
            .get(drag.workspace)
            .filter(|workspace| !workspace.linked_worktree)
            .map(|workspace| workspace.id.clone())
        else {
            return WorkspacePanelEffect::None;
        };
        for group in &mut self.groups {
            group.workspace_ids.retain(|id| id != &workspace_id);
        }
        if let WorkspaceDropTarget::Group(index) = target {
            let Some(group) = self.groups.get_mut(index) else {
                return WorkspacePanelEffect::None;
            };
            group.workspace_ids.push(workspace_id);
            group.expanded = true;
        }
        self.ensure_visible_selection();
        match self.persist_groups() {
            Ok(()) => WorkspacePanelEffect::None,
            Err(error) => WorkspacePanelEffect::Notice(error),
        }
    }

    pub(crate) fn workspace_drag_target(&self) -> Option<WorkspaceDropTarget> {
        self.workspace_drag.and_then(|drag| drag.target)
    }

    pub(crate) fn is_dragging_workspace(&self) -> bool {
        self.workspace_drag.is_some()
    }

    fn child_workspace_indices(&self, parent_id: &str) -> Vec<usize> {
        self.workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| workspace.parent_workspace_id.as_deref() == Some(parent_id))
            .map(|(index, _)| index)
            .collect()
    }

    fn reconcile_group_workspace_ids(&mut self) -> bool {
        let valid_workspace_ids = self
            .workspaces
            .iter()
            .filter(|workspace| !workspace.linked_worktree)
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>();
        let mut changed = false;
        for group in &mut self.groups {
            let previous_len = group.workspace_ids.len();
            group
                .workspace_ids
                .retain(|id| valid_workspace_ids.contains(&id.as_str()));
            changed |= group.workspace_ids.len() != previous_len;
        }
        changed
    }

    fn persist_groups(&self) -> Result<(), String> {
        let Some(path) = self.groups_path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create Hunkle config directory: {error}"))?;
        }
        let groups = self
            .groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "name": group.name,
                    "expanded": group.expanded,
                    "workspace_ids": group.workspace_ids,
                })
            })
            .collect::<Vec<_>>();
        let content = serde_json::to_string_pretty(&serde_json::json!({ "groups": groups }))
            .map_err(|error| format!("Could not serialize workspace groups: {error}"))?;
        fs::write(path, format!("{content}\n"))
            .map_err(|error| format!("Could not save workspace groups: {error}"))
    }

    fn focus_selected(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if let Some(workspace_id) = self
            .workspaces
            .get(selected)
            .map(|workspace| workspace.id.clone())
        {
            self.start_workspace_focus(workspace_id);
            return;
        }
        let agent_index = selected.saturating_sub(self.workspaces.len());
        let Some(agent) = self.agents.get(agent_index) else {
            return;
        };
        self.start_action(vec![
            "tab".to_owned(),
            "focus".to_owned(),
            agent.tab_id.clone(),
        ]);
    }

    fn start_workspace_focus(&mut self, workspace_id: String) {
        let request_id = self.mark_workspace_focus_pending(workspace_id.clone());
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result =
                run_herdr(&["workspace".to_owned(), "focus".to_owned(), workspace_id]).map(|_| ());
            let _ = sender.send(Completion::WorkspaceFocus { request_id, result });
        });
    }

    fn mark_workspace_focus_pending(&mut self, workspace_id: String) -> u64 {
        self.next_focus_request_id = self.next_focus_request_id.wrapping_add(1);
        let request_id = self.next_focus_request_id;
        self.pending_workspace_focus = Some(PendingWorkspaceFocus {
            request_id,
            workspace_id,
        });
        request_id
    }

    fn reconcile_pending_workspace_focus(&mut self) {
        let Some(pending) = self.pending_workspace_focus.as_ref() else {
            return;
        };
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == pending.workspace_id)
        else {
            self.pending_workspace_focus = None;
            return;
        };
        if workspace.focused {
            self.pending_workspace_focus = None;
        }
    }

    fn start_action(&self, args: Vec<String>) {
        self.start_action_with_reopen(args, None);
    }

    fn start_action_with_reopen(&self, args: Vec<String>, reopen_path: Option<PathBuf>) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = run_herdr(&args).map(|_| ());
            let _ = sender.send(Completion::Action {
                result,
                reopen_path,
            });
        });
    }

    fn move_selection(&mut self, delta: isize) {
        let selections = self.visible_selections();
        if selections.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .and_then(|selected| selections.iter().position(|entry| *entry == selected))
            .unwrap_or(0);
        self.selected = selections
            .get(
                current
                    .saturating_add_signed(delta)
                    .min(selections.len() - 1),
            )
            .copied();
    }

    fn visible_selections(&self) -> Vec<usize> {
        self.rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspacePanelRow::Workspace(index) => Some(index),
                WorkspacePanelRow::Agent(index) => {
                    Some(self.workspaces.len().saturating_add(index))
                }
                _ => None,
            })
            .collect()
    }

    fn ensure_visible_selection(&mut self) {
        let selections = self.visible_selections();
        if !self
            .selected
            .is_some_and(|selected| selections.contains(&selected))
        {
            self.selected = selections.first().copied();
        }
    }

    fn start_snapshot(&mut self) {
        self.loading = true;
        self.next_refresh = Instant::now() + REFRESH_INTERVAL;
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = run_herdr(&["api".to_owned(), "snapshot".to_owned()])
                .and_then(|value| parse_snapshot(&value))
                .map(|(mut workspaces, agents)| {
                    populate_workspace_branches(&mut workspaces);
                    (workspaces, agents)
                });
            let _ = sender.send(Completion::Snapshot(result));
        });
    }

    fn selection_key(&self) -> Option<SelectionKey> {
        let selected = self.selected?;
        self.workspaces
            .get(selected)
            .map(|workspace| SelectionKey::Workspace(workspace.id.clone()))
            .or_else(|| {
                self.agents
                    .get(selected.saturating_sub(self.workspaces.len()))
                    .map(|agent| SelectionKey::Agent(agent.pane_id.clone()))
            })
    }

    fn restore_selection(&mut self, previous: Option<SelectionKey>) {
        self.selected = previous
            .and_then(|key| match key {
                SelectionKey::Workspace(id) => self
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == id),
                SelectionKey::Agent(id) => self
                    .agents
                    .iter()
                    .position(|agent| agent.pane_id == id)
                    .map(|index| self.workspaces.len().saturating_add(index)),
            })
            .or_else(|| {
                let host = self.host_workspace_id.as_deref()?;
                self.workspaces
                    .iter()
                    .position(|workspace| workspace.id == host)
            })
            .or_else(|| {
                self.workspaces
                    .iter()
                    .position(|workspace| workspace.focused)
            })
            .or_else(|| (self.entry_count() > 0).then_some(0));
        self.ensure_visible_selection();
        self.scroll = self.scroll.min(self.visual_row_count().saturating_sub(1));
    }

    fn select_host_workspace(&mut self) {
        let Some(host) = self.host_workspace_id.as_deref() else {
            return;
        };
        if let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == host)
        {
            self.selected = Some(index);
        }
    }

    pub(crate) fn visual_row_count(&self) -> usize {
        self.rows().len()
    }

    pub(crate) fn selected_visual_row(&self) -> Option<usize> {
        let selected = self.selected?;
        self.rows().iter().position(|row| match row {
            WorkspacePanelRow::Workspace(index) => *index == selected,
            WorkspacePanelRow::Agent(index) => {
                self.workspaces.len().saturating_add(*index) == selected
            }
            _ => false,
        })
    }

    #[cfg(test)]
    pub(crate) fn ready_for_test(value: &Value) -> Self {
        let mut panel = Self::new(true, None);
        panel.placement = WorkspacePanelPlacement::Left;
        let (workspaces, agents) = parse_snapshot(value).unwrap();
        panel.workspaces = workspaces;
        panel.agents = agents;
        panel.restore_selection(None);
        panel
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspacePanelEffect {
    None,
    Unhandled,
    Close,
    Cycle,
    CreateWorkspace,
    CreateWorktree(String),
    CloseWorkspace(String),
    DeleteWorktree {
        workspace_id: String,
        path: Option<PathBuf>,
        parent_path: Option<PathBuf>,
    },
    OpenWorkspace(PathBuf),
    Notice(String),
}

fn sort_groups(groups: &mut [WorkspaceGroup]) {
    groups.sort_by_cached_key(|group| group.name.to_lowercase());
}

fn workspace_create_args(path: Option<&Path>) -> Vec<String> {
    let mut args = vec!["workspace".to_owned(), "create".to_owned()];
    if let Some(path) = path {
        args.push("--cwd".to_owned());
        args.push(path.to_string_lossy().into_owned());
    }
    args.push("--no-focus".to_owned());
    args
}

fn worktree_create_args(workspace_id: &str) -> Vec<String> {
    [
        "worktree",
        "create",
        "--workspace",
        workspace_id,
        "--no-focus",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn workspace_close_args(workspace_id: &str) -> Vec<String> {
    ["workspace", "close", workspace_id]
        .map(str::to_owned)
        .to_vec()
}

fn worktree_remove_args(workspace_id: &str) -> Vec<String> {
    ["worktree", "remove", "--workspace", workspace_id]
        .map(str::to_owned)
        .to_vec()
}

fn load_groups(path: &Path) -> Vec<WorkspaceGroup> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    value
        .get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| {
            Some(WorkspaceGroup {
                name: group.get("name")?.as_str()?.to_owned(),
                expanded: group
                    .get("expanded")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                workspace_ids: group
                    .get("workspace_ids")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

fn run_herdr(args: &[String]) -> Result<Value, String> {
    let output = Command::new("herdr")
        .args(args)
        .output()
        .map_err(|error| format!("Herdr unavailable: {error}"))?;
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().find(|line| !line.trim().is_empty());
        detail.map_or_else(
            || format!("Could not read Herdr response: {error}"),
            |detail| detail.trim().to_owned(),
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Herdr command failed")
            .to_owned());
    }
    if !output.status.success() {
        return Err("Herdr command failed".to_owned());
    }
    Ok(value)
}

fn parse_snapshot(value: &Value) -> Result<(Vec<HerdrWorkspace>, Vec<HerdrAgent>), String> {
    let snapshot = value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .ok_or_else(|| "Herdr returned an invalid session snapshot".to_owned())?;
    let mut workspaces: Vec<HerdrWorkspace> = snapshot
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "Herdr snapshot has no workspaces".to_owned())?
        .iter()
        .filter_map(|workspace| parse_workspace(workspace, snapshot))
        .collect();
    assign_worktree_parents(&mut workspaces);
    let agents = snapshot
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_agent)
        .collect();
    Ok((workspaces, agents))
}

fn parse_workspace(value: &Value, snapshot: &Value) -> Option<HerdrWorkspace> {
    let worktree = value.get("worktree").filter(|value| value.is_object());
    Some(HerdrWorkspace {
        id: value.get("workspace_id")?.as_str()?.to_owned(),
        label: value.get("label")?.as_str()?.to_owned(),
        path: workspace_path(value, snapshot),
        branch: None,
        parent_workspace_id: None,
        pane_count: value.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as usize,
        focused: value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: AgentStatus::parse(value.get("agent_status").and_then(Value::as_str)),
        repo_key: worktree
            .and_then(|worktree| worktree.get("repo_key"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        repo_root: worktree
            .and_then(|worktree| worktree.get("repo_root"))
            .and_then(Value::as_str)
            .map(PathBuf::from),
        linked_worktree: worktree
            .and_then(|worktree| worktree.get("is_linked_worktree"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn assign_worktree_parents(workspaces: &mut [HerdrWorkspace]) {
    let parent_ids = workspaces
        .iter()
        .map(|worktree| {
            if !worktree.linked_worktree {
                return None;
            }
            let repo_key = worktree.repo_key.as_deref()?;
            let exact_root = workspaces.iter().find(|candidate| {
                !candidate.linked_worktree
                    && candidate.path.as_deref() == worktree.repo_root.as_deref()
            });
            exact_root
                .or_else(|| {
                    workspaces.iter().find(|candidate| {
                        !candidate.linked_worktree
                            && candidate.repo_key.as_deref() == Some(repo_key)
                    })
                })
                .map(|parent| parent.id.clone())
        })
        .collect::<Vec<_>>();
    for (workspace, parent_id) in workspaces.iter_mut().zip(parent_ids) {
        workspace.parent_workspace_id = parent_id;
    }
}

fn populate_workspace_branches(workspaces: &mut [HerdrWorkspace]) {
    for workspace in workspaces {
        workspace.branch = workspace.path.as_deref().and_then(workspace_branch);
    }
}

fn workspace_branch(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let mut directory = if path.is_dir() { path } else { path.parent()? };
    loop {
        let dot_git = directory.join(".git");
        if dot_git.is_dir() {
            return branch_from_head(&dot_git.join("HEAD"));
        }
        if dot_git.is_file() {
            let git_file = fs::read_to_string(&dot_git).ok()?;
            let git_dir = git_file.trim().strip_prefix("gitdir:")?.trim();
            let git_dir = Path::new(git_dir);
            let git_dir = if git_dir.is_absolute() {
                git_dir.to_path_buf()
            } else {
                directory.join(git_dir)
            };
            return branch_from_head(&git_dir.join("HEAD"));
        }
        directory = directory.parent()?;
    }
}

fn branch_from_head(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .strip_prefix("ref: refs/heads/")
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
}

fn workspace_path(workspace: &Value, snapshot: &Value) -> Option<PathBuf> {
    if let Some(path) = workspace
        .get("worktree")
        .and_then(|worktree| worktree.get("checkout_path"))
        .and_then(Value::as_str)
    {
        return Some(PathBuf::from(path));
    }

    let workspace_id = workspace.get("workspace_id")?.as_str()?;
    let active_tab_id = workspace.get("active_tab_id").and_then(Value::as_str);
    let panes = snapshot.get("panes").and_then(Value::as_array)?;
    let focused_pane_id = snapshot
        .get("layouts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|layout| {
            layout.get("workspace_id").and_then(Value::as_str) == Some(workspace_id)
                && layout.get("tab_id").and_then(Value::as_str) == active_tab_id
        })
        .and_then(|layout| layout.get("focused_pane_id"))
        .and_then(Value::as_str);
    let workspace_panes = || {
        panes
            .iter()
            .filter(|pane| pane.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
    };
    let pane = focused_pane_id
        .and_then(|focused| {
            workspace_panes()
                .find(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(focused))
        })
        .or_else(|| {
            active_tab_id.and_then(|active_tab| {
                workspace_panes()
                    .find(|pane| pane.get("tab_id").and_then(Value::as_str) == Some(active_tab))
            })
        })
        .or_else(|| workspace_panes().next())?;
    pane.get("foreground_cwd")
        .and_then(Value::as_str)
        .or_else(|| pane.get("cwd").and_then(Value::as_str))
        .map(PathBuf::from)
}

fn parse_agent(value: &Value) -> Option<HerdrAgent> {
    Some(HerdrAgent {
        name: value.get("agent")?.as_str()?.to_owned(),
        workspace_id: value.get("workspace_id")?.as_str()?.to_owned(),
        tab_id: value.get("tab_id")?.as_str()?.to_owned(),
        pane_id: value.get("pane_id")?.as_str()?.to_owned(),
        focused: value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: AgentStatus::parse(value.get("agent_status").and_then(Value::as_str)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Value {
        serde_json::json!({
            "result": {
                "snapshot": {
                    "workspaces": [
                        {
                            "workspace_id": "w1",
                            "label": "HUNKLE",
                            "active_tab_id": "w1:t1",
                            "number": 2,
                            "pane_count": 2,
                            "focused": true,
                            "agent_status": "working"
                        },
                        {
                            "workspace_id": "w2",
                            "label": "docs",
                            "number": 3,
                            "pane_count": 1,
                            "focused": false,
                            "agent_status": "idle"
                        }
                    ],
                    "agents": [{
                        "agent": "opencode",
                        "agent_status": "blocked",
                        "focused": true,
                        "pane_id": "w1:p1",
                        "tab_id": "w1:t1",
                        "workspace_id": "w1"
                    }],
                    "panes": [{
                        "pane_id": "w1:p1",
                        "tab_id": "w1:t1",
                        "workspace_id": "w1",
                        "cwd": "/home/spoon/code/gitui",
                        "foreground_cwd": "/home/spoon/code/gitui"
                    }],
                    "layouts": [{
                        "workspace_id": "w1",
                        "tab_id": "w1:t1",
                        "focused_pane_id": "w1:p1"
                    }]
                }
            }
        })
    }

    #[test]
    fn parses_snapshot_and_tracks_workspace_and_agent_selection() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        assert_eq!(panel.workspaces.len(), 2);
        assert_eq!(panel.agents.len(), 1);
        assert_eq!(panel.workspaces[0].status, AgentStatus::Working);
        assert_eq!(
            panel.workspaces[0].path.as_deref(),
            Some(Path::new("/home/spoon/code/gitui"))
        );
        let mut worktree_snapshot = snapshot();
        worktree_snapshot["result"]["snapshot"]["workspaces"][0]["worktree"] =
            serde_json::json!({ "checkout_path": "/tmp/hunkle-worktree" });
        let (workspaces, _) = parse_snapshot(&worktree_snapshot).unwrap();
        assert_eq!(
            workspaces[0].path.as_deref(),
            Some(Path::new("/tmp/hunkle-worktree"))
        );
        assert_eq!(panel.agents[0].status, AgentStatus::Blocked);
        assert_eq!(panel.selected, Some(0));
        assert_eq!(panel.selected_visual_row(), Some(1));

        panel.move_selection(2);
        assert_eq!(panel.selected, Some(2));
        assert_eq!(panel.selected_visual_row(), Some(5));
        panel.move_selection(1);
        assert_eq!(panel.selected, Some(2));

        assert_eq!(
            panel.click_workspace(0),
            WorkspacePanelEffect::OpenWorkspace(PathBuf::from("/home/spoon/code/gitui"))
        );
    }

    #[test]
    fn keeps_the_target_workspace_active_until_herdr_confirms_focus() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.next_refresh = Instant::now() + Duration::from_secs(60);
        assert!(panel.select_workspace(1));
        panel.mark_workspace_focus_pending("w2".to_owned());

        assert!(!panel.workspace_is_active(0));
        assert!(panel.workspace_is_active(1));

        let stale = parse_snapshot(&snapshot()).unwrap();
        panel.sender.send(Completion::Snapshot(Ok(stale))).unwrap();
        let (changed, error, _) = panel.poll();
        assert!(changed);
        assert!(error.is_none());
        assert_eq!(panel.selected, Some(1));
        assert!(!panel.workspace_is_active(0));
        assert!(panel.workspace_is_active(1));
        assert!(panel.pending_workspace_focus.is_some());

        let mut confirmed = snapshot();
        confirmed["result"]["snapshot"]["workspaces"][0]["focused"] = false.into();
        confirmed["result"]["snapshot"]["workspaces"][1]["focused"] = true.into();
        panel
            .sender
            .send(Completion::Snapshot(
                Ok(parse_snapshot(&confirmed).unwrap()),
            ))
            .unwrap();
        panel.poll();

        assert!(panel.pending_workspace_focus.is_none());
        assert!(!panel.workspace_is_active(0));
        assert!(panel.workspace_is_active(1));
    }

    #[test]
    fn prehighlights_the_workspace_that_hosts_this_hunkle_process() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.host_workspace_id = Some("w2".to_owned());
        panel.restore_selection(None);

        assert!(!panel.workspace_is_active(0));
        assert!(panel.workspace_is_active(1));
        assert_eq!(panel.selected, Some(1));

        let stale = parse_snapshot(&snapshot()).unwrap();
        panel.workspaces = stale.0;
        assert!(!panel.workspace_is_active(0));
        assert!(panel.workspace_is_active(1));
    }

    #[test]
    fn prepares_the_hidden_process_cursor_after_focusing_away() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.host_workspace_id = Some("w1".to_owned());
        assert!(panel.select_workspace(1));
        panel.loading = true;
        let request_id = panel.mark_workspace_focus_pending("w2".to_owned());
        panel
            .sender
            .send(Completion::WorkspaceFocus {
                request_id,
                result: Ok(()),
            })
            .unwrap();

        panel.poll();

        assert!(panel.pending_workspace_focus.is_none());
        assert_eq!(panel.selected, Some(0));
        assert_eq!(panel.selected_workspace_id(), Some("w1"));
        assert!(panel.workspace_is_active(0));
    }

    #[test]
    fn rolls_back_only_the_current_failed_workspace_focus() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.loading = true;
        let old_request = panel.mark_workspace_focus_pending("w2".to_owned());
        let current_request = panel.mark_workspace_focus_pending("w1".to_owned());
        panel
            .sender
            .send(Completion::WorkspaceFocus {
                request_id: old_request,
                result: Err("old failure".to_owned()),
            })
            .unwrap();
        let (_, error, _) = panel.poll();
        assert!(error.is_none());
        assert_eq!(
            panel
                .pending_workspace_focus
                .as_ref()
                .map(|pending| pending.request_id),
            Some(current_request)
        );
        assert!(panel.workspace_is_active(0));

        panel
            .sender
            .send(Completion::WorkspaceFocus {
                request_id: current_request,
                result: Err("focus failed".to_owned()),
            })
            .unwrap();
        let (_, error, _) = panel.poll();
        assert_eq!(error.as_deref(), Some("focus failed"));
        assert!(panel.pending_workspace_focus.is_none());
        assert!(panel.workspace_is_active(0));
        assert!(!panel.workspace_is_active(1));
    }

    #[test]
    fn builds_background_workspace_and_worktree_commands() {
        assert_eq!(
            workspace_create_args(Some(Path::new("/tmp/current workspace"))),
            [
                "workspace",
                "create",
                "--cwd",
                "/tmp/current workspace",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            worktree_create_args("w1"),
            ["worktree", "create", "--workspace", "w1", "--no-focus",].map(str::to_owned)
        );
        assert_eq!(
            workspace_close_args("w1"),
            ["workspace", "close", "w1"].map(str::to_owned)
        );
        assert_eq!(
            worktree_remove_args("w3"),
            ["worktree", "remove", "--workspace", "w3"].map(str::to_owned)
        );
    }

    #[test]
    fn confirms_workspace_close_or_linked_worktree_removal() {
        let mut value = snapshot();
        value["result"]["snapshot"]["workspaces"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "workspace_id": "w3",
                "label": "feature-worktree",
                "pane_count": 1,
                "focused": false,
                "agent_status": "idle",
                "worktree": {
                    "checkout_path": "/tmp/worktrees/feature",
                    "is_linked_worktree": true,
                    "repo_key": "/home/spoon/code/gitui/.git",
                    "repo_root": "/home/spoon/code/gitui"
                }
            }));
        let mut panel = WorkspacePanel::ready_for_test(&value);

        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert_eq!(
            panel.delete_dialog.as_ref().map(|dialog| &dialog.kind),
            Some(&WorkspaceDeleteKind::Workspace { pane_count: 2 })
        );
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert!(panel.delete_dialog.is_none());

        panel.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WorkspacePanelEffect::CloseWorkspace("w1".to_owned())
        );
        assert!(panel.delete_dialog.is_none());

        assert!(panel.select_workspace(2));
        panel.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(
            panel.delete_dialog.as_ref().map(|dialog| &dialog.kind),
            Some(&WorkspaceDeleteKind::Worktree {
                path: Some(PathBuf::from("/tmp/worktrees/feature")),
                parent_path: Some(PathBuf::from("/home/spoon/code/gitui")),
            })
        );
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            WorkspacePanelEffect::DeleteWorktree {
                workspace_id: "w3".to_owned(),
                path: Some(PathBuf::from("/tmp/worktrees/feature")),
                parent_path: Some(PathBuf::from("/home/spoon/code/gitui")),
            }
        );

        assert!(panel.select_agent(0));
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            WorkspacePanelEffect::Notice("Select a workspace to close".to_owned())
        );
    }

    #[test]
    fn reopens_the_parent_only_after_successful_worktree_removal() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.next_refresh = Instant::now() + Duration::from_secs(60);
        panel.loading = true;
        let parent = PathBuf::from("/home/spoon/code/gitui");

        panel
            .sender
            .send(Completion::Action {
                result: Ok(()),
                reopen_path: Some(parent.clone()),
            })
            .unwrap();
        let (changed, error, reopen_path) = panel.poll();
        assert!(changed);
        assert_eq!(error, None);
        assert_eq!(reopen_path, Some(parent.clone()));

        panel.next_refresh = Instant::now() + Duration::from_secs(60);
        panel
            .sender
            .send(Completion::Action {
                result: Err("worktree has uncommitted changes".to_owned()),
                reopen_path: Some(parent),
            })
            .unwrap();
        let (changed, error, reopen_path) = panel.poll();
        assert!(changed);
        assert_eq!(error.as_deref(), Some("worktree has uncommitted changes"));
        assert_eq!(reopen_path, None);
    }

    #[test]
    fn create_menu_requires_a_selected_workspace_for_worktrees() {
        let mut panel = WorkspacePanel::ready_for_test(&snapshot());
        panel.toggle_create_menu();
        assert!(panel.create_menu_open);
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert_eq!(panel.create_menu_choice, 1);
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WorkspacePanelEffect::CreateWorktree("w1".to_owned())
        );
        assert!(!panel.create_menu_open);

        assert!(panel.select_agent(0));
        panel.toggle_create_menu();
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert_eq!(panel.create_menu_choice, 0);
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WorkspacePanelEffect::CreateWorkspace
        );
    }

    #[test]
    fn reads_branches_from_repositories_and_linked_worktrees() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let nested = repository.join("src/nested");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            repository.join(".git/HEAD"),
            "ref: refs/heads/feature/panel\n",
        )
        .unwrap();
        assert_eq!(workspace_branch(&nested).as_deref(), Some("feature/panel"));

        let worktree = directory.path().join("worktree");
        let git_dir = directory.path().join("git-data");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../git-data\n").unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/topic/worktree\n").unwrap();
        assert_eq!(
            workspace_branch(&worktree).as_deref(),
            Some("topic/worktree")
        );
    }

    #[test]
    fn nests_linked_worktrees_under_their_parent_and_prevents_dragging_them() {
        let mut value = snapshot();
        let workspaces = value["result"]["snapshot"]["workspaces"]
            .as_array_mut()
            .unwrap();
        workspaces.push(serde_json::json!({
            "workspace_id": "w3",
            "label": "feature-worktree",
            "pane_count": 1,
            "focused": false,
            "agent_status": "idle",
            "worktree": {
                "checkout_path": "/tmp/worktrees/feature",
                "is_linked_worktree": true,
                "repo_key": "/home/spoon/code/gitui/.git",
                "repo_root": "/home/spoon/code/gitui"
            }
        }));
        let mut panel = WorkspacePanel::ready_for_test(&value);

        assert_eq!(
            panel.workspaces[2].parent_workspace_id.as_deref(),
            Some("w1")
        );
        assert_eq!(
            &panel.rows()[..4],
            &[
                WorkspacePanelRow::Header,
                WorkspacePanelRow::Workspace(0),
                WorkspacePanelRow::Workspace(2),
                WorkspacePanelRow::Workspace(1),
            ]
        );
        assert_eq!(panel.workspace_indent(2), " ");
        assert!(!panel.begin_workspace_drag(2));

        panel.groups = vec![
            WorkspaceGroup {
                name: "Project".to_owned(),
                expanded: true,
                workspace_ids: vec!["w1".to_owned()],
            },
            WorkspaceGroup {
                name: "Old worktree group".to_owned(),
                expanded: true,
                workspace_ids: vec!["w3".to_owned()],
            },
        ];
        assert!(panel.reconcile_group_workspace_ids());
        assert!(panel.groups[1].workspace_ids.is_empty());
        assert_eq!(panel.group_for_workspace(2), Some(0));
        assert_eq!(panel.workspace_indent(2), "  ");
        assert_eq!(
            &panel.rows()[..5],
            &[
                WorkspacePanelRow::Header,
                WorkspacePanelRow::Group(0),
                WorkspacePanelRow::Workspace(0),
                WorkspacePanelRow::Workspace(2),
                WorkspacePanelRow::Group(1),
            ]
        );
    }

    #[test]
    fn persists_groups_and_moves_workspaces_between_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace-groups.json");
        let mut panel = WorkspacePanel::new(true, Some(path.clone()));
        assert_eq!(panel.placement, WorkspacePanelPlacement::Left);
        panel.cycle_placement();
        assert_eq!(panel.placement, WorkspacePanelPlacement::Right);
        panel.cycle_placement();
        assert_eq!(panel.placement, WorkspacePanelPlacement::Off);
        panel.cycle_placement();
        assert_eq!(panel.placement, WorkspacePanelPlacement::Left);
        let (workspaces, agents) = parse_snapshot(&snapshot()).unwrap();
        panel.workspaces = workspaces;
        panel.agents = agents;
        panel.restore_selection(None);

        panel.begin_group();
        panel.paste("Zulu work");
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert!(panel.begin_workspace_drag(0));
        panel.update_workspace_drag(Some(WorkspaceDropTarget::Group(0)));
        assert_eq!(panel.finish_workspace_drag(), WorkspacePanelEffect::None);
        assert_eq!(panel.group_for_workspace(0), Some(0));
        assert!(path.exists());

        assert!(panel.begin_workspace_drag(1));
        panel.update_workspace_drag(Some(WorkspaceDropTarget::Group(0)));
        assert_eq!(panel.finish_workspace_drag(), WorkspacePanelEffect::None);
        assert_eq!(panel.group_for_workspace(1), Some(0));

        panel.begin_group();
        panel.paste("alpha work");
        assert_eq!(
            panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WorkspacePanelEffect::None
        );
        assert_eq!(panel.groups[0].name, "alpha work");
        assert_eq!(panel.groups[1].name, "Zulu work");
        assert!(panel.begin_workspace_drag(0));
        panel.update_workspace_drag(Some(WorkspaceDropTarget::Group(0)));
        assert_eq!(panel.finish_workspace_drag(), WorkspacePanelEffect::None);
        assert_eq!(panel.group_for_workspace(0), Some(0));
        assert_eq!(panel.groups[1].workspace_ids, ["w2"]);

        panel.toggle_group(1);
        assert!(!panel.groups[1].expanded);
        assert!(!panel.rows().contains(&WorkspacePanelRow::Workspace(1)));

        let restored = WorkspacePanel::new(true, Some(path));
        assert_eq!(restored.groups[0].name, "alpha work");
        assert_eq!(restored.groups[0].workspace_ids, ["w1"]);
        assert_eq!(restored.groups[1].name, "Zulu work");
        assert!(!restored.groups[1].expanded);
        assert_eq!(restored.groups[1].workspace_ids, ["w2"]);
    }
}
