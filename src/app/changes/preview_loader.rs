use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use crate::git::{self, Change};

pub(super) struct PreviewLoader {
    generation: u64,
    sender: Sender<Request>,
    receiver: Receiver<Completion>,
}

impl PreviewLoader {
    pub(super) fn new() -> Self {
        let (sender, request_rx) = mpsc::channel::<Request>();
        let (result_tx, receiver) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut request) = request_rx.recv() {
                while let Ok(latest) = request_rx.try_recv() {
                    request = latest;
                }
                let content = match &request.task {
                    Task::File(path) => git::file_content(&request.root, path),
                    Task::Commit(oid) => git::commit_diff(&request.root, oid),
                    Task::Diff(change) => git::diff(&request.root, change),
                }
                .unwrap_or_else(|error| error.to_string());
                if result_tx
                    .send(Completion {
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
        Self {
            generation: 0,
            sender,
            receiver,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) fn request_file(&mut self, root: &Path, path: String) {
        self.request(root, Task::File(path));
    }

    pub(super) fn request_commit(&mut self, root: &Path, oid: String) {
        self.request(root, Task::Commit(oid));
    }

    pub(super) fn request_diff(&mut self, root: &Path, change: Change) {
        self.request(root, Task::Diff(change));
    }

    pub(super) fn poll(&mut self, active_root: Option<&Path>) -> Option<String> {
        let mut content = None;
        while let Ok(result) = self.receiver.try_recv() {
            if result.generation == self.generation
                && active_root.is_some_and(|root| root == result.root)
            {
                content = Some(result.content);
            }
        }
        content
    }

    fn request(&mut self, root: &Path, task: Task) {
        self.invalidate();
        let _ = self.sender.send(Request {
            generation: self.generation,
            root: root.to_path_buf(),
            task,
        });
    }
}

struct Request {
    generation: u64,
    root: PathBuf,
    task: Task,
}

enum Task {
    File(String),
    Commit(String),
    Diff(Change),
}

struct Completion {
    generation: u64,
    root: PathBuf,
    content: String,
}
