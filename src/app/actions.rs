use crate::git::CommandOutput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionId {
    Push,
    Fetch,
    PullRebase,
    Custom,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActionItem {
    pub(crate) id: ActionId,
    pub(crate) label: &'static str,
    pub(crate) detail: &'static str,
}

pub(crate) const ACTION_ITEMS: [ActionItem; 4] = [
    ActionItem {
        id: ActionId::Push,
        label: "Push",
        detail: "git push",
    },
    ActionItem {
        id: ActionId::Fetch,
        label: "Fetch",
        detail: "all remotes",
    },
    ActionItem {
        id: ActionId::PullRebase,
        label: "Pull --rebase",
        detail: "update branch",
    },
    ActionItem {
        id: ActionId::Custom,
        label: "Run Git command...",
        detail: "non-interactive",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandStatus {
    Input,
    Running,
    Complete {
        success: bool,
        exit_code: Option<i32>,
    },
}

pub(crate) struct CommandRecord {
    pub(crate) command: String,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
}

pub(crate) struct ActionsState {
    pub(crate) selection: usize,
    pub(crate) input: String,
    pub(crate) command: String,
    pub(crate) status: CommandStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) transcript: Vec<CommandRecord>,
    pub(crate) scroll: u16,
    pub(crate) scroll_max: u16,
}

impl Default for ActionsState {
    fn default() -> Self {
        Self {
            selection: 0,
            input: String::new(),
            command: String::new(),
            status: CommandStatus::Input,
            stdout: String::new(),
            stderr: String::new(),
            transcript: Vec::new(),
            scroll: 0,
            scroll_max: 0,
        }
    }
}

impl ActionsState {
    pub(crate) fn selected(&self) -> ActionId {
        ACTION_ITEMS[self.selection.min(ACTION_ITEMS.len() - 1)].id
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let len = ACTION_ITEMS.len() as isize;
        self.selection = (self.selection as isize + delta).rem_euclid(len) as usize;
    }

    pub(crate) fn begin_input(&mut self) {
        self.input.clear();
        self.command.clear();
        self.stdout.clear();
        self.stderr.clear();
        self.transcript.clear();
        self.scroll = 0;
        self.scroll_max = 0;
        self.status = CommandStatus::Input;
    }

    pub(crate) fn begin_command(&mut self, command: String) {
        self.command = command;
        self.stdout.clear();
        self.stderr.clear();
        self.scroll = u16::MAX;
        self.scroll_max = 0;
        self.status = CommandStatus::Running;
    }

    pub(crate) fn complete(&mut self, output: CommandOutput) {
        self.input.clear();
        self.stdout = output.stdout;
        self.stderr = output.stderr;
        self.transcript.push(CommandRecord {
            command: self.command.clone(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            success: output.success,
            exit_code: output.exit_code,
        });
        self.scroll = u16::MAX;
        self.status = CommandStatus::Complete {
            success: output.success,
            exit_code: output.exit_code,
        };
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.input.clear();
        self.stdout.clear();
        self.stderr = error;
        self.transcript.push(CommandRecord {
            command: self.command.clone(),
            stdout: String::new(),
            stderr: self.stderr.clone(),
            success: false,
            exit_code: None,
        });
        self.scroll = u16::MAX;
        self.status = CommandStatus::Complete {
            success: false,
            exit_code: None,
        };
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) {
        self.scroll = if delta > 0 {
            self.scroll
                .saturating_add(delta as u16)
                .min(self.scroll_max)
        } else {
            self.scroll.saturating_sub(delta.unsigned_abs() as u16)
        };
    }
}

pub(crate) fn action_command(action: ActionId) -> Option<(&'static str, Vec<String>)> {
    match action {
        ActionId::Push => Some(("Push", vec!["push".to_owned()])),
        ActionId::Fetch => Some((
            "Fetch",
            vec!["fetch".to_owned(), "--all".to_owned(), "--prune".to_owned()],
        )),
        ActionId::PullRebase => Some((
            "Pull --rebase",
            vec!["pull".to_owned(), "--rebase".to_owned()],
        )),
        ActionId::Custom => None,
    }
}

pub(crate) fn parse_git_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote = None;
    let mut escaped = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            started = true;
            escaped = false;
            continue;
        }
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {
                current.push(character);
                started = true;
            }
            (Some('"'), '\\') => escaped = true,
            (Some('"'), _) => {
                current.push(character);
                started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '\\') => {
                escaped = true;
                started = true;
            }
            (None, character) if character.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(character);
                started = true;
            }
            _ => unreachable!(),
        }
    }

    if escaped {
        return Err("Command ends with an unfinished escape".to_owned());
    }
    if quote.is_some() {
        return Err("Command contains an unterminated quote".to_owned());
    }
    if started {
        args.push(current);
    }
    if args.first().is_some_and(|argument| argument == "git") {
        args.remove(0);
    }
    if args.is_empty() {
        return Err("Enter a Git command".to_owned());
    }
    Ok(args)
}

pub(crate) fn display_git_command(args: &[String]) -> String {
    let arguments = args
        .iter()
        .map(|argument| {
            if argument.chars().all(|character| {
                character.is_ascii_alphanumeric() || "-_=./:@,".contains(character)
            }) {
                argument.clone()
            } else {
                format!("'{0}'", argument.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("git {arguments}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_custom_git_arguments_without_a_shell() {
        assert_eq!(
            parse_git_args("git log --format='hello world' -- README.md").unwrap(),
            ["log", "--format=hello world", "--", "README.md"]
        );
        assert_eq!(parse_git_args("show \"\"").unwrap(), ["show", ""]);
        assert_eq!(
            display_git_command(&["commit".to_owned(), "hello world".to_owned()]),
            "git commit 'hello world'"
        );
        assert!(parse_git_args("commit -m 'unfinished").is_err());
        assert!(parse_git_args("git").is_err());
    }
}
