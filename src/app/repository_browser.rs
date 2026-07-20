use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use ratatui::widgets::ListState;
use serde_json::Value;

use crate::git::Branch;

const REMOTE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

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

#[derive(Debug, Clone)]
pub(crate) struct RemoteItems<T> {
    items: Vec<T>,
    fetched_at: Option<Instant>,
    loading: bool,
    error: Option<String>,
}

impl<T> RemoteItems<T> {
    pub(crate) fn count(&self) -> Option<usize> {
        self.fetched_at.map(|_| self.items.len())
    }

    pub(crate) fn items(&self) -> Option<&[T]> {
        self.fetched_at.map(|_| self.items.as_slice())
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn start_if_stale(&mut self, now: Instant) -> bool {
        if self.loading
            || self.fetched_at.is_some_and(|fetched_at| {
                now.saturating_duration_since(fetched_at) < REMOTE_CACHE_TTL
            })
        {
            return false;
        }
        self.loading = true;
        self.error = None;
        true
    }

    fn finish(&mut self, result: Result<Vec<T>, String>, now: Instant) {
        self.loading = false;
        match result {
            Ok(items) => {
                self.items = items;
                self.fetched_at = Some(now);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn paused(&self) -> Self
    where
        T: Clone,
    {
        let mut cached = self.clone();
        cached.loading = false;
        cached
    }

    #[cfg(test)]
    pub(crate) fn ready(items: Vec<T>) -> Self {
        Self {
            items,
            fetched_at: Some(Instant::now()),
            loading: false,
            error: None,
        }
    }
}

impl<T> Default for RemoteItems<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            fetched_at: None,
            loading: false,
            error: None,
        }
    }
}

#[derive(Clone, Default)]
struct RemoteCache {
    pull_requests: RemoteItems<PullRequest>,
    issues: RemoteItems<Issue>,
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
    cache: HashMap<PathBuf, RemoteCache>,
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
            pull_requests: RemoteItems::default(),
            issues: RemoteItems::default(),
            root: None,
            cache: HashMap::new(),
            generation: 0,
            sender,
            receiver,
        }
    }
}

impl RepositoryBrowser {
    pub(crate) fn open(&mut self, root: &Path, branches: &[Branch], prefetch: bool) {
        self.tab = BrowserTab::Branches;
        self.query.clear();
        self.branches = branches.to_vec();
        self.select_first();
        self.activate_root(root);
        if prefetch {
            self.refresh_all();
        }
    }

    pub(crate) fn prefetch(&mut self, root: &Path) {
        self.activate_root(root);
        self.refresh_all();
    }

    fn activate_root(&mut self, root: &Path) {
        if self.root.as_deref() == Some(root) {
            return;
        }
        if let Some(previous_root) = self.root.take() {
            self.cache.insert(
                previous_root,
                RemoteCache {
                    pull_requests: self.pull_requests.paused(),
                    issues: self.issues.paused(),
                },
            );
        }
        self.root = Some(root.to_owned());
        self.generation = self.generation.wrapping_add(1);
        let cached = self.cache.get(root).cloned().unwrap_or_default();
        self.pull_requests = cached.pull_requests;
        self.issues = cached.issues;
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(completion) = self.receiver.try_recv() {
            if completion.generation != self.generation {
                continue;
            }
            let now = Instant::now();
            match completion.result {
                RemoteResult::PullRequests(result) => {
                    self.pull_requests.finish(result, now);
                }
                RemoteResult::Issues(result) => {
                    self.issues.finish(result, now);
                }
            }
            changed = true;
        }
        if changed {
            self.clamp_selection();
        }
        changed
    }

    pub(crate) fn set_tab(&mut self, tab: BrowserTab) {
        self.tab = tab;
        match tab {
            BrowserTab::PullRequests => self.refresh(RemoteKind::PullRequests),
            BrowserTab::Issues => self.refresh(RemoteKind::Issues),
            BrowserTab::Branches => {}
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

    pub(crate) fn selected_branch(&self) -> Option<&Branch> {
        if self.tab != BrowserTab::Branches {
            return None;
        }
        let filtered_index = self.state.selected()?;
        let branch_index = *self.branch_indices().get(filtered_index)?;
        self.branches.get(branch_index)
    }

    pub(crate) fn pull_request_indices(&self) -> Vec<usize> {
        self.pull_requests.items().map_or_else(Vec::new, |items| {
            matching_indices(&self.query, items, |item| {
                format!(
                    "{} {} {} {}",
                    item.number, item.title, item.branch, item.author
                )
            })
        })
    }

    pub(crate) fn issue_indices(&self) -> Vec<usize> {
        self.issues.items().map_or_else(Vec::new, |items| {
            matching_indices(&self.query, items, |item| {
                format!(
                    "{} {} {} {}",
                    item.number, item.title, item.author, item.labels
                )
            })
        })
    }

    fn select_first(&mut self) {
        self.state = ListState::default();
        self.state.select((self.result_count() > 0).then_some(0));
    }

    fn clamp_selection(&mut self) {
        let count = self.result_count();
        if count == 0 {
            self.state.select(None);
        } else {
            self.state
                .select(Some(self.state.selected().unwrap_or(0).min(count - 1)));
        }
    }

    fn refresh_all(&mut self) {
        self.refresh(RemoteKind::PullRequests);
        self.refresh(RemoteKind::Issues);
    }

    fn refresh(&mut self, kind: RemoteKind) {
        let should_start = match kind {
            RemoteKind::PullRequests => self.pull_requests.start_if_stale(Instant::now()),
            RemoteKind::Issues => self.issues.start_if_stale(Instant::now()),
        };
        if should_start && let Some(root) = self.root.clone() {
            self.start_remote_load(&root, kind);
        }
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
            false,
        );
        assert_eq!(browser.pull_requests.count(), None);
        assert!(!browser.pull_requests.is_loading());
        browser.push('x');
        assert_eq!(browser.result_count(), 0);
        browser.clear();
        assert_eq!(browser.result_count(), 1);
    }

    #[test]
    fn keeps_stale_items_visible_while_refreshing_and_after_errors() {
        let now = Instant::now();
        let mut items = RemoteItems {
            items: vec![PullRequest {
                number: 42,
                title: "Cached pull request".to_owned(),
                branch: "feature/cache".to_owned(),
                author: "octo".to_owned(),
                draft: false,
            }],
            fetched_at: Some(now - REMOTE_CACHE_TTL),
            loading: false,
            error: None,
        };

        assert!(items.start_if_stale(now));
        assert_eq!(items.count(), Some(1));
        assert_eq!(items.items().unwrap()[0].number, 42);
        items.finish(Err("offline".to_owned()), now);
        assert_eq!(items.count(), Some(1));
        assert_eq!(items.error(), Some("offline"));
    }

    #[test]
    fn caches_remote_items_per_repository() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut browser = RepositoryBrowser::default();
        browser.activate_root(first.path());
        browser.pull_requests = RemoteItems::ready(vec![PullRequest {
            number: 7,
            title: "Remember me".to_owned(),
            branch: "feature/cache".to_owned(),
            author: "octo".to_owned(),
            draft: false,
        }]);

        browser.activate_root(second.path());
        assert_eq!(browser.pull_requests.count(), None);
        browser.activate_root(first.path());
        assert_eq!(browser.pull_requests.count(), Some(1));
        assert_eq!(browser.pull_requests.items().unwrap()[0].number, 7);
    }
}
