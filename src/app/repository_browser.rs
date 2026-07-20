use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use ratatui::widgets::ListState;
use serde_json::Value;

use crate::git::Branch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserTab {
    Branches,
    PullRequests,
    Issues,
}

impl BrowserTab {
    pub(crate) const ALL: [Self; 3] = [Self::Branches, Self::PullRequests, Self::Issues];

    fn index(self) -> usize {
        match self {
            Self::Branches => 0,
            Self::PullRequests => 1,
            Self::Issues => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) branch: String,
    pub(crate) author: String,
    pub(crate) draft: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) labels: String,
}

#[derive(Debug)]
pub(crate) enum RemoteItems<T> {
    NotLoaded,
    Loading,
    Ready(Vec<T>),
    Error(String),
}

impl<T> RemoteItems<T> {
    pub(crate) fn count(&self) -> Option<usize> {
        match self {
            Self::Ready(items) => Some(items.len()),
            Self::NotLoaded | Self::Loading | Self::Error(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
enum RemoteKind {
    PullRequests,
    Issues,
}

enum RemoteResult {
    PullRequests(Result<Vec<PullRequest>, String>),
    Issues(Result<Vec<Issue>, String>),
}

struct RemoteCompletion {
    generation: u64,
    result: RemoteResult,
}

pub(crate) struct RepositoryBrowser {
    pub(crate) tab: BrowserTab,
    pub(crate) query: String,
    pub(crate) state: ListState,
    pub(crate) branches: Vec<Branch>,
    pub(crate) pull_requests: RemoteItems<PullRequest>,
    pub(crate) issues: RemoteItems<Issue>,
    root: Option<PathBuf>,
    generation: u64,
    sender: Sender<RemoteCompletion>,
    receiver: Receiver<RemoteCompletion>,
}

impl Default for RepositoryBrowser {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            tab: BrowserTab::Branches,
            query: String::new(),
            state: ListState::default(),
            branches: Vec::new(),
            pull_requests: RemoteItems::NotLoaded,
            issues: RemoteItems::NotLoaded,
            root: None,
            generation: 0,
            sender,
            receiver,
        }
    }
}

impl RepositoryBrowser {
    pub(crate) fn open(&mut self, root: &Path, branches: &[Branch]) {
        self.tab = BrowserTab::Branches;
        self.query.clear();
        self.branches = branches.to_vec();
        self.select_first();

        if self.root.as_deref() == Some(root) {
            return;
        }
        self.root = Some(root.to_owned());
        self.generation = self.generation.wrapping_add(1);
        self.pull_requests = RemoteItems::NotLoaded;
        self.issues = RemoteItems::NotLoaded;
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(completion) = self.receiver.try_recv() {
            if completion.generation != self.generation {
                continue;
            }
            match completion.result {
                RemoteResult::PullRequests(result) => {
                    self.pull_requests = result
                        .map(RemoteItems::Ready)
                        .unwrap_or_else(RemoteItems::Error);
                }
                RemoteResult::Issues(result) => {
                    self.issues = result
                        .map(RemoteItems::Ready)
                        .unwrap_or_else(RemoteItems::Error);
                }
            }
            changed = true;
        }
        changed
    }

    pub(crate) fn set_tab(&mut self, tab: BrowserTab) {
        self.tab = tab;
        let remote_kind = match tab {
            BrowserTab::PullRequests if matches!(self.pull_requests, RemoteItems::NotLoaded) => {
                self.pull_requests = RemoteItems::Loading;
                Some(RemoteKind::PullRequests)
            }
            BrowserTab::Issues if matches!(self.issues, RemoteItems::NotLoaded) => {
                self.issues = RemoteItems::Loading;
                Some(RemoteKind::Issues)
            }
            BrowserTab::Branches | BrowserTab::PullRequests | BrowserTab::Issues => None,
        };
        if let (Some(root), Some(kind)) = (self.root.as_deref(), remote_kind) {
            self.start_remote_load(root, kind);
        }
        self.select_first();
    }

    pub(crate) fn move_tab(&mut self, delta: isize) {
        let index = self
            .tab
            .index()
            .saturating_add_signed(delta)
            .min(BrowserTab::ALL.len() - 1);
        self.set_tab(BrowserTab::ALL[index]);
    }

    pub(crate) fn push(&mut self, character: char) {
        self.query.push(character);
        self.select_first();
    }

    pub(crate) fn paste(&mut self, text: &str) {
        self.query.extend(
            text.chars()
                .filter(|character| !matches!(character, '\r' | '\n')),
        );
        self.select_first();
    }

    pub(crate) fn backspace(&mut self) {
        self.query.pop();
        self.select_first();
    }

    pub(crate) fn clear(&mut self) {
        self.query.clear();
        self.select_first();
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let count = self.result_count();
        if count == 0 {
            self.state.select(None);
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        self.state
            .select(Some(current.saturating_add_signed(delta).min(count - 1)));
    }

    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.result_count() {
            return false;
        }
        self.state.select(Some(index));
        true
    }

    pub(crate) fn result_count(&self) -> usize {
        match self.tab {
            BrowserTab::Branches => self.branch_indices().len(),
            BrowserTab::PullRequests => self.pull_request_indices().len(),
            BrowserTab::Issues => self.issue_indices().len(),
        }
    }

    pub(crate) fn branch_indices(&self) -> Vec<usize> {
        matching_indices(&self.query, &self.branches, |branch| {
            format!(
                "{} {} {} {} {}",
                branch.name, branch.upstream, branch.oid, branch.date, branch.subject
            )
        })
    }

    pub(crate) fn pull_request_indices(&self) -> Vec<usize> {
        match &self.pull_requests {
            RemoteItems::Ready(items) => matching_indices(&self.query, items, |item| {
                format!(
                    "{} {} {} {}",
                    item.number, item.title, item.branch, item.author
                )
            }),
            RemoteItems::NotLoaded | RemoteItems::Loading | RemoteItems::Error(_) => Vec::new(),
        }
    }

    pub(crate) fn issue_indices(&self) -> Vec<usize> {
        match &self.issues {
            RemoteItems::Ready(items) => matching_indices(&self.query, items, |item| {
                format!(
                    "{} {} {} {}",
                    item.number, item.title, item.author, item.labels
                )
            }),
            RemoteItems::NotLoaded | RemoteItems::Loading | RemoteItems::Error(_) => Vec::new(),
        }
    }

    fn select_first(&mut self) {
        self.state = ListState::default();
        self.state.select((self.result_count() > 0).then_some(0));
    }

    fn start_remote_load(&self, root: &Path, kind: RemoteKind) {
        let root = root.to_owned();
        let generation = self.generation;
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = match kind {
                RemoteKind::PullRequests => RemoteResult::PullRequests(load_pull_requests(&root)),
                RemoteKind::Issues => RemoteResult::Issues(load_issues(&root)),
            };
            let _ = sender.send(RemoteCompletion { generation, result });
        });
    }
}

fn matching_indices<T>(query: &str, items: &[T], text: impl Fn(&T) -> String) -> Vec<usize> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .collect();
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let candidate = text(item).to_lowercase();
            terms
                .iter()
                .all(|term| candidate.contains(term))
                .then_some(index)
        })
        .collect()
}

fn load_pull_requests(root: &Path) -> Result<Vec<PullRequest>, String> {
    let value = run_gh(
        root,
        &[
            "pr",
            "list",
            "--limit",
            "100",
            "--json",
            "number,title,headRefName,author,isDraft",
        ],
    )?;
    let items = value
        .as_array()
        .ok_or_else(|| "GitHub CLI returned invalid pull request data".to_owned())?;
    Ok(items
        .iter()
        .filter_map(|item| {
            Some(PullRequest {
                number: item.get("number")?.as_u64()?,
                title: item.get("title")?.as_str()?.to_owned(),
                branch: item.get("headRefName")?.as_str()?.to_owned(),
                author: author_login(item),
                draft: item
                    .get("isDraft")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect())
}

fn load_issues(root: &Path) -> Result<Vec<Issue>, String> {
    let value = run_gh(
        root,
        &[
            "issue",
            "list",
            "--limit",
            "100",
            "--json",
            "number,title,author,labels",
        ],
    )?;
    let items = value
        .as_array()
        .ok_or_else(|| "GitHub CLI returned invalid issue data".to_owned())?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let labels = item
                .get("labels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|label| label.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            Some(Issue {
                number: item.get("number")?.as_u64()?,
                title: item.get("title")?.as_str()?.to_owned(),
                author: author_login(item),
                labels,
            })
        })
        .collect())
}

fn author_login(item: &Value) -> String {
    item.get("author")
        .and_then(|author| author.get("login"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn run_gh(root: &Path, args: &[&str]) -> Result<Value, String> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|error| format!("GitHub CLI unavailable: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr)
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Could not load GitHub data")
            .trim()
            .to_owned();
        return Err(error);
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Could not read GitHub CLI output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_each_tab_and_parses_remote_items() {
        let pull_requests: Value = serde_json::from_str(
            r#"[{"number":42,"title":"Branch browser","headRefName":"feature/browser","author":{"login":"octo"},"isDraft":true}]"#,
        )
        .unwrap();
        let issues: Value = serde_json::from_str(
            r#"[{"number":7,"title":"Keyboard navigation","author":null,"labels":[{"name":"ux"}]}]"#,
        )
        .unwrap();

        let pulls = pull_requests.as_array().unwrap();
        let pull = &pulls[0];
        assert_eq!(pull.get("headRefName").unwrap(), "feature/browser");
        assert_eq!(author_login(pull), "octo");
        let issues = issues.as_array().unwrap();
        assert_eq!(author_login(&issues[0]), "unknown");

        let indices = matching_indices("branch octo", pulls, |item| item.to_string());
        assert_eq!(indices, [0]);

        let directory = tempfile::tempdir().unwrap();
        let mut browser = RepositoryBrowser::default();
        browser.open(
            directory.path(),
            &[Branch {
                name: "feature/browser".to_owned(),
                upstream: "origin/feature/browser".to_owned(),
                oid: "abc1234".to_owned(),
                date: "2026-07-20".to_owned(),
                subject: "Add repository browser".to_owned(),
                remote: false,
                current: true,
            }],
        );
        assert!(matches!(browser.pull_requests, RemoteItems::NotLoaded));
        browser.push('x');
        assert_eq!(browser.result_count(), 0);
        browser.clear();
        assert_eq!(browser.result_count(), 1);
    }
}
