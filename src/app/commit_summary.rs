use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use crate::git::{self, DiffSummary};

struct SummaryResult {
    generation: u64,
    root: PathBuf,
    requested: Vec<String>,
    result: Result<HashMap<String, DiffSummary>, String>,
}

pub(crate) struct CommitSummaryCache {
    root: Option<PathBuf>,
    generation: u64,
    summaries: HashMap<String, DiffSummary>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    sender: Sender<SummaryResult>,
    receiver: Receiver<SummaryResult>,
}

impl Default for CommitSummaryCache {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            root: None,
            generation: 0,
            summaries: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            sender,
            receiver,
        }
    }
}

impl CommitSummaryCache {
    pub(crate) fn get(&self, oid: &str) -> Option<&DiffSummary> {
        self.summaries.get(oid)
    }

    pub(crate) fn failed(&self, oid: &str) -> bool {
        self.failed.contains(oid)
    }

    pub(crate) fn request<'a>(&mut self, root: &Path, oids: impl IntoIterator<Item = &'a str>) {
        self.activate(root);
        let mut requested = Vec::new();
        let mut seen = HashSet::new();
        for oid in oids {
            if !self.summaries.contains_key(oid)
                && !self.pending.contains(oid)
                && !self.failed.contains(oid)
                && seen.insert(oid.to_owned())
            {
                requested.push(oid.to_owned());
            }
        }
        if requested.is_empty() {
            return;
        }
        self.pending.extend(requested.iter().cloned());
        let generation = self.generation;
        let root = root.to_owned();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result =
                git::commit_summaries(&root, &requested).map_err(|error| error.to_string());
            let _ = sender.send(SummaryResult {
                generation,
                root,
                requested,
                result,
            });
        });
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(done) = self.receiver.try_recv() {
            if done.generation != self.generation || self.root.as_deref() != Some(&done.root) {
                continue;
            }
            for oid in &done.requested {
                self.pending.remove(oid);
            }
            match done.result {
                Ok(summaries) => {
                    self.failed.extend(
                        done.requested
                            .iter()
                            .filter(|oid| !summaries.contains_key(*oid))
                            .cloned(),
                    );
                    self.summaries.extend(summaries);
                }
                Err(_) => self.failed.extend(done.requested),
            }
            changed = true;
        }
        changed
    }

    fn activate(&mut self, root: &Path) {
        if self.root.as_deref() == Some(root) {
            return;
        }
        self.root = Some(root.to_owned());
        self.generation = self.generation.wrapping_add(1);
        self.summaries.clear();
        self.pending.clear();
        self.failed.clear();
    }
}
