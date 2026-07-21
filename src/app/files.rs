use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use crate::{
    filesystem::{FileOperation, validate_name},
    git::Change,
};

use super::{App, LeftPane, Mode, TextInput, View};

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
    DiscardUnstaged {
        change: Change,
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

pub(crate) struct FileDrag {
    pub(super) source: super::changes::ExplorerEntry,
    pub(super) start: Position,
    pub(super) active: bool,
    pub(super) target: Option<String>,
}

impl App {
    pub(super) fn handle_file_dialog(&mut self, key: KeyEvent) {
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
            FileDialogKind::DiscardUnstaged { .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => self.close_file_dialog(),
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_discard_unstaged(),
                _ => {}
            },
        }
    }

    pub(super) fn open_add_dialog(&mut self) {
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

    pub(super) fn open_rename_dialog(&mut self) {
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

    pub(super) fn open_delete_dialog(&mut self) {
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

    pub(super) fn open_discard_unstaged_dialog(&mut self) {
        if self.changes.pane != LeftPane::Worktree || self.changes.history_focused {
            return;
        }
        let Some(change) = self
            .session
            .data()
            .and_then(|repo| {
                self.changes
                    .selected_change_index(repo)
                    .map(|index| (repo, index))
            })
            .and_then(|(repo, index)| repo.changes.get(index))
            .cloned()
        else {
            return;
        };
        if change.staged {
            self.notice = Some("Select an unstaged change to discard".to_owned());
            return;
        }
        self.file_dialog = Some(FileDialog {
            kind: FileDialogKind::DiscardUnstaged { change },
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

    fn confirm_discard_unstaged(&mut self) {
        let Some(FileDialogKind::DiscardUnstaged { change }) =
            self.file_dialog.as_ref().map(|dialog| dialog.kind.clone())
        else {
            return;
        };
        self.close_file_dialog();
        if !self.session.start_discard_unstaged(change) {
            self.notice = Some("Another repository operation is running".to_owned());
        }
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

    pub(super) fn handle_file_dialog_click(&mut self, point: Position) {
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
                Some(FileDialogKind::DiscardUnstaged { .. }) => self.confirm_discard_unstaged(),
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

    pub(super) fn begin_file_drag(&mut self, point: Position) -> bool {
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

    pub(super) fn update_file_drag(&mut self, point: Position) {
        let mut target = self.file_drop_target_at(point);
        if let Some(drag) = &mut self.file_drag {
            drag.active |= drag.start != point;
            if drag.source.is_directory && target.as_deref() == Some(&drag.source.path) {
                target = None;
            }
            drag.target = target;
        }
    }

    pub(super) fn finish_file_drag(&mut self, point: Position) {
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
