use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

use crate::git::{self, CommandOutput, RepositoryData};

const STATUS_INTERVAL: Duration = Duration::from_millis(800);

pub(crate) enum WorkerCompletion {
    Commit(Result<CommandOutput, String>),
    Fetch(Result<CommandOutput, String>),
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
    result: Result<u64, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerKind {
    Commit,
    Fetch,
}

pub(crate) struct RepositorySession {
    data: Option<RepositoryData>,
    commit_running: bool,
    fetch_running: bool,
    worker_tx: Sender<WorkerResult>,
    worker_rx: Receiver<WorkerResult>,
    status_tx: Sender<StatusResult>,
    status_rx: Receiver<StatusResult>,
    status_check_running: bool,
    status_signature: Option<u64>,
    next_fetch_at: Instant,
    next_status_check: Instant,
}

impl RepositorySession {
    pub(crate) fn new(path: &Path, fetch_interval: Duration) -> Self {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let data = git::load(path).ok();
        let status_signature = data
            .as_ref()
            .and_then(|repository| git::worktree_signature(&repository.root).ok());

        Self {
            data,
            commit_running: false,
            fetch_running: false,
            worker_tx,
            worker_rx,
            status_tx,
            status_rx,
            status_check_running: false,
            status_signature,
            next_fetch_at: Instant::now() + fetch_interval,
            next_status_check: Instant::now() + STATUS_INTERVAL,
        }
    }

    pub(crate) fn data(&self) -> Option<&RepositoryData> {
        self.data.as_ref()
    }

    pub(crate) fn commit_running(&self) -> bool {
        self.commit_running
    }

    pub(crate) fn fetch_running(&self) -> bool {
        self.fetch_running
    }

    pub(crate) fn open(&mut self, path: &Path, fetch_interval: Duration) -> Result<()> {
        let data = git::load(path)?;
        self.status_signature = git::worktree_signature(&data.root).ok();
        self.next_status_check = Instant::now() + STATUS_INTERVAL;
        self.next_fetch_at = Instant::now() + fetch_interval;
        self.data = Some(data);
        Ok(())
    }

    pub(crate) fn reload(&mut self) -> Result<()> {
        let Some(root) = self.data.as_ref().map(|repository| repository.root.clone()) else {
            bail!("No repository selected");
        };
        let data = git::load(&root)?;
        self.status_signature = git::worktree_signature(&data.root).ok();
        self.data = Some(data);
        Ok(())
    }

    pub(crate) fn reset_fetch_deadline(&mut self, fetch_interval: Duration) {
        self.next_fetch_at = Instant::now() + fetch_interval;
    }

    pub(crate) fn start_commit(&mut self, message: String) -> bool {
        if self.commit_running {
            return false;
        }
        let Some(root) = self.data.as_ref().map(|repository| repository.root.clone()) else {
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

    pub(crate) fn maybe_start_fetch(&mut self, enabled: bool, fetch_interval: Duration) {
        if !enabled || self.fetch_running || Instant::now() < self.next_fetch_at {
            return;
        }
        let Some(root) = self.data.as_ref().map(|repository| repository.root.clone()) else {
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
            || self.commit_running
            || self.fetch_running
            || Instant::now() < self.next_status_check
        {
            return;
        }
        let Some(root) = self.data.as_ref().map(|repository| repository.root.clone()) else {
            return;
        };

        self.status_check_running = true;
        self.next_status_check = Instant::now() + STATUS_INTERVAL;
        let baseline = self.status_signature;
        let sender = self.status_tx.clone();
        thread::spawn(move || {
            let result = git::worktree_signature(&root).map_err(|error| error.to_string());
            let _ = sender.send(StatusResult {
                root,
                baseline,
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
                    return true;
                }
            }
        }
        false
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
    fn ignores_status_result_from_a_previous_repository() {
        let mut session = session("/active", Some(10));
        session.status_check_running = true;
        session
            .status_tx
            .send(StatusResult {
                root: PathBuf::from("/previous"),
                baseline: Some(10),
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
                result: Ok(30),
            })
            .unwrap();

        assert!(!session.next_worktree_change());
        assert_eq!(session.status_signature, Some(20));
        assert!(!session.status_check_running);
    }

    fn session(root: &str, status_signature: Option<u64>) -> RepositorySession {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        RepositorySession {
            data: Some(RepositoryData {
                root: PathBuf::from(root),
                branch: "main".to_owned(),
                changes: Vec::new(),
                files: Vec::new(),
                history: Vec::new(),
                commits: Vec::new(),
            }),
            commit_running: false,
            fetch_running: false,
            worker_tx,
            worker_rx,
            status_tx,
            status_rx,
            status_check_running: false,
            status_signature,
            next_fetch_at: Instant::now(),
            next_status_check: Instant::now(),
        }
    }
}
