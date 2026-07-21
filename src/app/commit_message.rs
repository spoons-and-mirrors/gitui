use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Instant,
};

use crate::diagnostics;

const MODEL: &str = "openai/gpt-5.6-sol";
const VARIANT: &str = "low";
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_MESSAGE_BYTES: usize = 2_000;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffSource {
    Staged,
    Unstaged,
}

impl DiffSource {
    fn label(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
        }
    }
}

pub(crate) struct CommitMessageCompletion {
    pub(crate) root: PathBuf,
    pub(crate) baseline: String,
    pub(crate) result: Result<String, String>,
}

pub(crate) struct CommitMessageGenerator {
    available: bool,
    running: bool,
    sender: Sender<CommitMessageCompletion>,
    receiver: Receiver<CommitMessageCompletion>,
}

impl CommitMessageGenerator {
    pub(crate) fn detect() -> Self {
        #[cfg(test)]
        let available = false;
        #[cfg(not(test))]
        let available = command_available("opencode");

        diagnostics::event(format!("commit message generator available={available}"));
        Self::new(available)
    }

    fn new(available: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            available,
            running: false,
            sender,
            receiver,
        }
    }

    #[cfg(test)]
    pub(crate) fn ready_for_test() -> Self {
        Self::new(true)
    }

    pub(crate) fn is_available(&self) -> bool {
        self.available
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn start(&mut self, root: PathBuf, baseline: String) -> Result<(), String> {
        if !self.available {
            return Err("OpenCode is not installed".to_owned());
        }
        if self.running {
            return Err("A commit message is already being generated".to_owned());
        }

        let sender = self.sender.clone();
        let worker_root = root.clone();
        let worker_baseline = baseline.clone();
        thread::Builder::new()
            .name("hunkle-commit-message".to_owned())
            .spawn(move || {
                let result = generate_message(&worker_root);
                let _ = sender.send(CommitMessageCompletion {
                    root: worker_root,
                    baseline: worker_baseline,
                    result,
                });
            })
            .map_err(|error| format!("Could not start OpenCode: {error}"))?;

        self.running = true;
        diagnostics::event(format!(
            "commit message generation requested root={}",
            root.display()
        ));
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Option<CommitMessageCompletion> {
        let completion = self.receiver.try_recv().ok()?;
        self.running = false;
        Some(completion)
    }
}

#[cfg(not(test))]
fn command_available(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        #[cfg(windows)]
        let candidates = [
            directory.join(format!("{name}.exe")),
            directory.join(format!("{name}.cmd")),
            directory.join(format!("{name}.bat")),
            directory.join(name),
        ];
        #[cfg(not(windows))]
        let candidates = [directory.join(name)];
        candidates.into_iter().any(|candidate| candidate.is_file())
    })
}

fn generate_message(root: &Path) -> Result<String, String> {
    let started = Instant::now();
    let (source, mut diff) = read_diff(root)?;
    let original_len = diff.len();
    if diff.len() > MAX_DIFF_BYTES {
        diff.truncate(MAX_DIFF_BYTES);
        diff.extend_from_slice(b"\n\n[Diff truncated by Hunkle]\n");
    }
    let temp_path = write_temp_diff(&diff)?;
    let args = opencode_args(&temp_path, source);
    let output = Command::new("opencode")
        .args(&args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Could not run OpenCode: {error}"));
    let _ = fs::remove_file(&temp_path);
    let output = output?;
    diagnostics::event(format!(
        "commit message generation finished root={} source={} diff_bytes={} elapsed_ms={} success={}",
        root.display(),
        source.label(),
        original_len,
        started.elapsed().as_millis(),
        output.status.success()
    ));
    if !output.status.success() {
        return Err(format!(
            "OpenCode could not generate a commit message: {}",
            concise_error(&output.stderr)
        ));
    }
    clean_message(&String::from_utf8_lossy(&output.stdout))
}

fn read_diff(root: &Path) -> Result<(DiffSource, Vec<u8>), String> {
    let staged = git_diff(root, true)?;
    if !staged.is_empty() {
        return Ok((DiffSource::Staged, staged));
    }
    let unstaged = git_diff(root, false)?;
    if unstaged.is_empty() {
        Err("No staged or unstaged diff to describe".to_owned())
    } else {
        Ok((DiffSource::Unstaged, unstaged))
    }
}

fn git_diff(root: &Path, staged: bool) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .arg("diff")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .arg("--no-ext-diff")
        .arg("--no-color");
    if staged {
        command.arg("--cached");
    }
    let output = command
        .arg("--")
        .output()
        .map_err(|error| format!("Could not inspect Git changes: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "Could not inspect Git changes: {}",
            concise_error(&output.stderr)
        ))
    }
}

fn write_temp_diff(diff: &[u8]) -> Result<PathBuf, String> {
    for _ in 0..8 {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hunkle-commit-diff-{}-{sequence}.patch",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(diff)
                    .map_err(|error| format!("Could not prepare diff for OpenCode: {error}"))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("Could not prepare diff for OpenCode: {error}"));
            }
        }
    }
    Err("Could not create a temporary diff for OpenCode".to_owned())
}

fn opencode_args(diff_path: &Path, source: DiffSource) -> Vec<OsString> {
    vec![
        "run".into(),
        "--pure".into(),
        "--model".into(),
        MODEL.into(),
        "--variant".into(),
        VARIANT.into(),
        "--file".into(),
        diff_path.as_os_str().to_owned(),
        "--title".into(),
        "Hunkle commit message".into(),
        commit_prompt(source).into(),
    ]
}

fn commit_prompt(source: DiffSource) -> String {
    format!(
        "Write a Git commit message for the attached {} diff. Output only the commit message as plain text: no Markdown fences, labels, analysis, or explanation. Use an imperative subject that explains the meaningful change, ideally 50-72 characters. Add a concise body of at most three short lines only when it adds useful context. Be specific but neither terse nor verbose. Do not invent changes that are absent from the diff.",
        source.label()
    )
}

fn clean_message(output: &str) -> Result<String, String> {
    let mut message = output.trim();
    if message.starts_with("```") && message.ends_with("```") {
        message = message
            .split_once('\n')
            .map_or(message, |(_, remainder)| remainder);
        message = message.strip_suffix("```").map_or(message, str::trim_end);
    }
    if message.is_empty() {
        return Err("OpenCode returned an empty commit message".to_owned());
    }
    let mut message = message.to_owned();
    if message.len() > MAX_MESSAGE_BYTES {
        let mut end = MAX_MESSAGE_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    Ok(message.trim().to_owned())
}

fn concise_error(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("unknown error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_low_reasoning_opencode_command() {
        let args = opencode_args(Path::new("/tmp/change.patch"), DiffSource::Staged);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            &args[..10],
            [
                "run",
                "--pure",
                "--model",
                "openai/gpt-5.6-sol",
                "--variant",
                "low",
                "--file",
                "/tmp/change.patch",
                "--title",
                "Hunkle commit message",
            ]
        );
        assert!(args[10].contains("attached staged diff"));
    }

    #[test]
    fn prefers_staged_changes_and_falls_back_to_unstaged() {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init"]);
        git(
            directory.path(),
            &["config", "user.email", "test@example.com"],
        );
        git(directory.path(), &["config", "user.name", "Test"]);
        fs::write(directory.path().join("file.txt"), "initial\n").unwrap();
        git(directory.path(), &["add", "file.txt"]);
        git(directory.path(), &["commit", "-m", "initial"]);

        fs::write(directory.path().join("file.txt"), "unstaged\n").unwrap();
        let (source, diff) = read_diff(directory.path()).unwrap();
        assert_eq!(source, DiffSource::Unstaged);
        assert!(String::from_utf8_lossy(&diff).contains("+unstaged"));

        git(directory.path(), &["add", "file.txt"]);
        fs::write(directory.path().join("file.txt"), "later unstaged\n").unwrap();
        let (source, diff) = read_diff(directory.path()).unwrap();
        assert_eq!(source, DiffSource::Staged);
        let diff = String::from_utf8_lossy(&diff);
        assert!(diff.contains("+unstaged"));
        assert!(!diff.contains("later unstaged"));
    }

    #[test]
    fn cleans_plain_and_fenced_messages() {
        assert_eq!(
            clean_message("  Improve workspace loading\n").unwrap(),
            "Improve workspace loading"
        );
        assert_eq!(
            clean_message("```text\nImprove workspace loading\n```\n").unwrap(),
            "Improve workspace loading"
        );
        assert!(clean_message("  ").is_err());
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
