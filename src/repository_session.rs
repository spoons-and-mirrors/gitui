use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::git::{self, Change, CommandOutput, RepositoryData};

const MIN_STATUS_INTERVAL: Duration = Duration::from_millis(800);
const MAX_STATUS_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) enum WorkerCompletion {
    Commit(Result<CommandOutput, String>),
    Fetch(Result<CommandOutput, String>),
    Command(CommandCompletion),
    Mutation(Result<(), String>),
}

pub(crate) enum Mutation {
    Stage(Change),
    Unstage(Change),
    StageAll,
    UnstageAll,
    StageHunk { patch: String, index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadKind {
    Open,
    Reload,
}

pub(crate) struct LoadCompletion {
    pub(crate) kind: LoadKind,
    pub(crate) result: Result<(), String>,
}

pub(crate) struct CommandCompletion {
    pub(crate) label: String,
    pub(crate) result: Result<CommandOutput, String>,
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
    activity_generation: u64,
    result: Result<u64, String>,
}

struct LoadResult {
    generation: u64,
    kind: LoadKind,
    fetch_interval: Duration,
    result: Result<(RepositoryData, Option<u64>), String>,
}

#[derive(Debug)]
enum WorkerKind {
    Commit,
    Fetch,
    Command { label: String },
    Mutation,
}

pub(crate) struct RepositorySession {
    data: Option<RepositoryData>,
    commit_running: bool,
    fetch_running: bool,
    command_running: bool,
    mutation_running: bool,
    worker_tx: Sender<WorkerResult>,
    worker_rx: Receiver<WorkerResult>,
    status_tx: Sender<StatusResult>,
    status_rx: Receiver<StatusResult>,
    status_check_running: bool,
    status_signature: Option<u64>,
    next_fetch_at: Instant,
    next_status_check: Instant,
    status_interval: Duration,
    status_activity_generation: u64,
    load_generation: u64,
    load_running: bool,
    load_tx: Sender<LoadResult>,
    load_rx: Receiver<LoadResult>,
}

impl RepositorySession {
    pub(crate) fn new(path: &Path, fetch_interval: Duration) -> Self {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let (load_tx, load_rx) = mpsc::channel();
        let data = git::load_or_local(path).ok();
        let status_signature = data
            .as_ref()
            .filter(|repository| !repository.is_local())
            .and_then(|repository| git::worktree_signature(&repository.root).ok());

        Self {
            data,
            commit_running: false,
            fetch_running: false,
            command_running: false,
            mutation_running: false,
            worker_tx,
            worker_rx,
            status_tx,
            status_rx,
            status_check_running: false,
            status_signature,
            next_fetch_at: Instant::now() + fetch_interval,
            next_status_check: Instant::now() + MIN_STATUS_INTERVAL,
            status_interval: MIN_STATUS_INTERVAL,
            status_activity_generation: 0,
            load_generation: 0,
            load_running: false,
            load_tx,
            load_rx,
        }
    }

    pub(crate) fn data(&self) -> Option<&RepositoryData> {
        self.data.as_ref()
    }

    fn git_root(&self) -> Option<PathBuf> {
        self.data
            .as_ref()
            .filter(|repository| !repository.is_local())
            .map(|repository| repository.root.clone())
    }

    pub(crate) fn commit_running(&self) -> bool {
        self.commit_running
    }

    pub(crate) fn fetch_running(&self) -> bool {
        self.fetch_running
    }

    pub(crate) fn command_running(&self) -> bool {
        self.command_running
    }

    pub(crate) fn start_open(&mut self, path: PathBuf, fetch_interval: Duration) -> bool {
        self.start_load(path, LoadKind::Open, fetch_interval)
    }

    pub(crate) fn start_reload(&mut self, fetch_interval: Duration) -> bool {
        let Some(root) = self.data.as_ref().map(|repository| repository.root.clone()) else {
            return false;
        };
        self.start_load(root, LoadKind::Reload, fetch_interval)
    }

    pub(crate) fn next_load_completion(&mut self) -> Option<LoadCompletion> {
        while let Ok(done) = self.load_rx.try_recv() {
            if done.generation != self.load_generation {
                continue;
            }
            self.load_running = false;
            let result = done.result.map(|(data, signature)| {
                self.status_signature = signature;
                self.reset_status_interval();
                if done.kind == LoadKind::Open {
                    self.next_fetch_at = Instant::now() + done.fetch_interval;
                }
                self.data = Some(data);
            });
            return Some(LoadCompletion {
                kind: done.kind,
                result,
            });
        }
        None
    }

    pub(crate) fn reset_fetch_deadline(&mut self, fetch_interval: Duration) {
        self.next_fetch_at = Instant::now() + fetch_interval;
    }

    pub(crate) fn start_commit(&mut self, message: String) -> bool {
        if self.commit_running || self.command_running || self.mutation_running {
            return false;
        }
        let Some(root) = self.git_root() else {
            return false;
        };

        self.commit_running = true;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = git::commit(&root, &message).map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult {
                kind: WorkerKind::Commit,
                root,
                result,
            });
        });
        true
    }

    pub(crate) fn start_command(&mut self, label: String, args: Vec<String>) -> bool {
        if self.command_running
            || self.commit_running
            || self.fetch_running
            || self.mutation_running
        {
            return false;
        }
        let Some(root) = self.git_root() else {
            return false;
        };

        self.command_running = true;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = git::run_command(&root, &args).map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult {
                kind: WorkerKind::Command { label },
                root,
                result,
            });
        });
        true
    }

    pub(crate) fn start_mutation(&mut self, mutation: Mutation) -> bool {
        if self.mutation_running
            || self.commit_running
            || self.fetch_running
            || self.command_running
            || self.load_running
        {
            return false;
        }
        let Some(root) = self.git_root() else {
            return false;
        };

        self.mutation_running = true;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = match &mutation {
                Mutation::Stage(change) => git::stage(&root, change),
                Mutation::Unstage(change) => git::unstage(&root, change),
                Mutation::StageAll => git::stage_all(&root),
                Mutation::UnstageAll => git::unstage_all(&root),
                Mutation::StageHunk { patch, index } => git::stage_hunk(&root, patch, *index),
            }
            .map(|()| CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                success: true,
                exit_code: Some(0),
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult {
                kind: WorkerKind::Mutation,
                root,
                result,
            });
        });
        true
    }

    pub(crate) fn maybe_start_fetch(&mut self, enabled: bool, fetch_interval: Duration) {
        if !enabled
            || self.load_running
            || self.fetch_running
            || self.command_running
            || self.mutation_running
            || Instant::now() < self.next_fetch_at
        {
            return;
        }
        let Some(root) = self.git_root() else {
            return;
        };

        self.fetch_running = true;
        self.next_fetch_at = Instant::now() + fetch_interval;
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

    pub(crate) fn maybe_start_status_check(&mut self) {
        if self.status_check_running
            || self.load_running
            || self.commit_running
            || self.fetch_running
            || self.command_running
            || self.mutation_running
            || Instant::now() < self.next_status_check
        {
            return;
        }
        let Some(root) = self.git_root() else {
            return;
        };

        self.status_check_running = true;
        let baseline = self.status_signature;
        let activity_generation = self.status_activity_generation;
        let sender = self.status_tx.clone();
        thread::spawn(move || {
            let result = git::worktree_signature(&root).map_err(|error| error.to_string());
            let _ = sender.send(StatusResult {
                root,
                baseline,
                activity_generation,
                result,
            });
        });
    }

    pub(crate) fn next_worker_completion(
        &mut self,
        fetch_interval: Duration,
    ) -> Option<WorkerCompletion> {
        while let Ok(done) = self.worker_rx.try_recv() {
            let active = self
                .data
                .as_ref()
                .is_some_and(|repository| repository.root == done.root);
            match done.kind {
                WorkerKind::Commit => {
                    self.commit_running = false;
                    if active {
                        return Some(WorkerCompletion::Commit(done.result));
                    }
                }
                WorkerKind::Fetch => {
                    self.fetch_running = false;
                    self.next_fetch_at = Instant::now() + fetch_interval;
                    if active {
                        return Some(WorkerCompletion::Fetch(done.result));
                    }
                }
                WorkerKind::Command { label } => {
                    self.command_running = false;
                    if active {
                        return Some(WorkerCompletion::Command(CommandCompletion {
                            label,
                            result: done.result,
                        }));
                    }
                }
                WorkerKind::Mutation => {
                    self.mutation_running = false;
                    if active {
                        return Some(WorkerCompletion::Mutation(done.result.map(|_| ())));
                    }
                }
            }
        }
        None
    }

    pub(crate) fn next_worktree_change(&mut self) -> bool {
        while let Ok(done) = self.status_rx.try_recv() {
            self.status_check_running = false;
            let active = self
                .data
                .as_ref()
                .is_some_and(|repository| repository.root == done.root);
            if !active || self.status_signature != done.baseline {
                continue;
            }
            if let Ok(signature) = done.result {
                let changed = self
                    .status_signature
                    .replace(signature)
                    .is_some_and(|previous| previous != signature);
                if changed {
                    self.reset_status_interval();
                    return true;
                }
            }
            if done.activity_generation == self.status_activity_generation {
                self.status_interval = self
                    .status_interval
                    .saturating_mul(2)
                    .min(MAX_STATUS_INTERVAL);
                self.next_status_check = Instant::now() + self.status_interval;
            }
        }
        false
    }

    pub(crate) fn note_activity(&mut self) {
        self.status_activity_generation = self.status_activity_generation.wrapping_add(1);
        self.status_interval = MIN_STATUS_INTERVAL;
        self.next_status_check = self
            .next_status_check
            .min(Instant::now() + MIN_STATUS_INTERVAL);
    }

    fn reset_status_interval(&mut self) {
        self.status_interval = MIN_STATUS_INTERVAL;
        self.next_status_check = Instant::now() + MIN_STATUS_INTERVAL;
    }

    fn start_load(&mut self, path: PathBuf, kind: LoadKind, fetch_interval: Duration) -> bool {
        if self.load_running && kind == LoadKind::Reload {
            return false;
        }
        self.load_generation = self.load_generation.wrapping_add(1);
        self.load_running = true;
        let generation = self.load_generation;
        let sender = self.load_tx.clone();
        thread::spawn(move || {
            let result = git::load_or_local(&path)
                .map(|data| {
                    let signature = (!data.is_local())
                        .then(|| git::worktree_signature(&data.root).ok())
                        .flatten();
                    (data, signature)
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(LoadResult {
                generation,
                kind,
                fetch_interval,
                result,
            });
        });
        true
    }

    #[cfg(test)]
    pub(crate) fn schedule_fetch_now(&mut self) {
        self.next_fetch_at = Instant::now();
    }

    #[cfg(test)]
    pub(crate) fn schedule_status_check_now(&mut self) {
        self.next_status_check = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_worker_completion_from_a_previous_repository() {
        let mut session = session("/active", Some(10));
        session.commit_running = true;
        session
            .worker_tx
            .send(WorkerResult {
                kind: WorkerKind::Commit,
                root: PathBuf::from("/previous"),
                result: Err("old result".to_owned()),
            })
            .unwrap();

        assert!(
            session
                .next_worker_completion(Duration::from_secs(60))
                .is_none()
        );
        assert!(!session.commit_running());
    }

    #[test]
    fn ignores_command_completion_from_a_previous_repository() {
        let mut session = session("/active", Some(10));
        session.command_running = true;
        session
            .worker_tx
            .send(WorkerResult {
                kind: WorkerKind::Command {
                    label: "Push".to_owned(),
                },
                root: PathBuf::from("/previous"),
                result: Err("old result".to_owned()),
            })
            .unwrap();

        assert!(
            session
                .next_worker_completion(Duration::from_secs(60))
                .is_none()
        );
        assert!(!session.command_running());
    }

    #[test]
    fn ignores_superseded_repository_loads() {
        let mut session = session("/active", Some(7));
        session.load_generation = 2;
        session.load_running = true;
        let mut stale_data = session.data.clone().unwrap();
        stale_data.root = PathBuf::from("/stale");
        session
            .load_tx
            .send(LoadResult {
                generation: 1,
                kind: LoadKind::Open,
                fetch_interval: Duration::ZERO,
                result: Ok((stale_data, Some(99))),
            })
            .unwrap();

        assert!(session.next_load_completion().is_none());
        assert_eq!(session.data().unwrap().root, Path::new("/active"));
        assert_eq!(session.status_signature, Some(7));
        assert!(session.load_running);
    }

    #[test]
    fn ignores_status_result_from_a_previous_repository() {
        let mut session = session("/active", Some(10));
        session.status_check_running = true;
        session
            .status_tx
            .send(StatusResult {
                root: PathBuf::from("/previous"),
                baseline: Some(10),
                activity_generation: 0,
                result: Ok(20),
            })
            .unwrap();

        assert!(!session.next_worktree_change());
        assert_eq!(session.status_signature, Some(10));
        assert!(!session.status_check_running);
    }

    #[test]
    fn ignores_status_result_with_a_superseded_baseline() {
        let mut session = session("/active", Some(20));
        session.status_check_running = true;
        session
            .status_tx
            .send(StatusResult {
                root: PathBuf::from("/active"),
                baseline: Some(10),
                activity_generation: 0,
                result: Ok(30),
            })
            .unwrap();

        assert!(!session.next_worktree_change());
        assert_eq!(session.status_signature, Some(20));
        assert!(!session.status_check_running);
    }

    #[test]
    fn local_workspaces_do_not_schedule_git_background_work() {
        let mut session = session("/local", None);
        session.data.as_mut().unwrap().kind = git::RepositoryKind::Local;
        session.schedule_fetch_now();
        session.schedule_status_check_now();

        session.maybe_start_fetch(true, Duration::ZERO);
        session.maybe_start_status_check();

        assert!(!session.fetch_running);
        assert!(!session.status_check_running);
        assert!(!session.start_commit("local".to_owned()));
        assert!(!session.start_command("Status".to_owned(), vec!["status".to_owned()]));
        assert!(!session.start_mutation(Mutation::StageAll));
    }

    #[test]
    fn activity_does_not_postpone_or_back_off_status_checks() {
        let mut session = session("/active", Some(10));
        session.next_status_check = Instant::now();
        session.note_activity();
        assert!(session.next_status_check <= Instant::now());

        session.status_check_running = true;
        session
            .status_tx
            .send(StatusResult {
                root: PathBuf::from("/active"),
                baseline: Some(10),
                activity_generation: 0,
                result: Ok(10),
            })
            .unwrap();
        assert!(!session.next_worktree_change());
        assert_eq!(session.status_interval, MIN_STATUS_INTERVAL);
    }

    fn session(root: &str, status_signature: Option<u64>) -> RepositorySession {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let (load_tx, load_rx) = mpsc::channel();
        RepositorySession {
            data: Some(RepositoryData {
                root: PathBuf::from(root),
                kind: git::RepositoryKind::Git,
                branch: "main".to_owned(),
                changes: Vec::new(),
                files: Vec::new(),
                history: Vec::new(),
                commits: Vec::new(),
                files_fingerprint: 0,
                changes_fingerprint: 0,
                change_counts: (0, 0),
                graph_width: 0,
                graph_truncated: false,
            }),
            commit_running: false,
            fetch_running: false,
            command_running: false,
            mutation_running: false,
            worker_tx,
            worker_rx,
            status_tx,
            status_rx,
            status_check_running: false,
            status_signature,
            next_fetch_at: Instant::now(),
            next_status_check: Instant::now(),
            status_interval: MIN_STATUS_INTERVAL,
            status_activity_generation: 0,
            load_generation: 0,
            load_running: false,
            load_tx,
            load_rx,
        }
    }
}
