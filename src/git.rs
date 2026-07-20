use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKind {
    Git,
    Local,
}

#[derive(Debug, Clone)]
pub struct RepositoryData {
    pub root: PathBuf,
    pub kind: RepositoryKind,
    pub branch: String,
    pub changes: Vec<Change>,
    pub files: Vec<String>,
    pub directories: Vec<String>,
    pub history: Vec<Commit>,
    pub commits: Vec<Commit>,
    pub files_fingerprint: u64,
    pub changes_fingerprint: u64,
    pub change_counts: (usize, usize),
    pub graph_width: usize,
    pub graph_truncated: bool,
    pub branches: Vec<Branch>,
    pub github_remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshScope(u8);

impl RefreshScope {
    const INVENTORY: Self = Self(1 << 1);
    const HISTORY: Self = Self(1 << 2);
    const GRAPH: Self = Self(1 << 3);
    const REFS: Self = Self(1 << 4);

    pub const WORKTREE: Self = Self(1);
    pub const WORKTREE_AND_INVENTORY: Self = Self(Self::WORKTREE.0 | Self::INVENTORY.0);
    pub const HISTORY_AND_REFS: Self = Self(Self::HISTORY.0 | Self::GRAPH.0 | Self::REFS.0);
    pub const ALL: Self =
        Self(Self::WORKTREE.0 | Self::INVENTORY.0 | Self::HISTORY.0 | Self::GRAPH.0 | Self::REFS.0);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn includes(self, facet: Self) -> bool {
        self.0 & facet.0 != 0
    }
}

#[derive(Debug)]
pub struct RepositoryUpdate {
    root: PathBuf,
    worktree: Option<WorktreeData>,
    inventory: Option<InventoryData>,
    history: Option<HistoryData>,
    graph: Option<GraphData>,
    refs: Option<RefsData>,
}

#[derive(Debug)]
struct WorktreeData {
    changes: Vec<Change>,
    fingerprint: u64,
    counts: (usize, usize),
}

#[derive(Debug)]
struct InventoryData {
    files: Vec<String>,
    directories: Vec<String>,
    fingerprint: u64,
}

#[derive(Debug)]
struct HistoryData {
    branch: String,
    commits: Vec<Commit>,
}

#[derive(Debug)]
struct GraphData {
    commits: Vec<Commit>,
    width: usize,
    truncated: bool,
}

#[derive(Debug)]
struct RefsData {
    branches: Vec<Branch>,
    github_remote: bool,
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub upstream: String,
    pub oid: String,
    pub date: String,
    pub subject: String,
    pub remote: bool,
    pub current: bool,
}

impl RepositoryData {
    pub fn is_local(&self) -> bool {
        self.kind == RepositoryKind::Local
    }

    pub(crate) fn apply(&mut self, update: RepositoryUpdate) {
        debug_assert_eq!(self.root, update.root);
        if let Some(worktree) = update.worktree {
            self.changes = worktree.changes;
            self.changes_fingerprint = worktree.fingerprint;
            self.change_counts = worktree.counts;
        }
        if let Some(inventory) = update.inventory {
            self.files = inventory.files;
            self.directories = inventory.directories;
            self.files_fingerprint = inventory.fingerprint;
        }
        if let Some(history) = update.history {
            self.branch = history.branch;
            self.history = history.commits;
        }
        if let Some(graph) = update.graph {
            self.commits = graph.commits;
            self.graph_width = graph.width;
            self.graph_truncated = graph.truncated;
        }
        if let Some(refs) = update.refs {
            self.branches = refs.branches;
            self.github_remote = refs.github_remote;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Change {
    pub path: String,
    pub original_path: Option<String>,
    pub code: char,
    pub staged: bool,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub graph: Vec<GraphCell>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub files: Vec<String>,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphCell {
    pub symbol: char,
    pub color: usize,
}

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub exit_code: Option<i32>,
}

pub fn discover(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| "could not start git; make sure it is installed")?;

    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if root.is_empty() {
        bail!("Git did not return a worktree path");
    }

    let root = fs::canonicalize(root).context("could not resolve repository root")?;
    let requested = fs::canonicalize(path).context("could not resolve requested directory")?;
    if root != requested {
        bail!(
            "{} is not a repository root (enclosing repository: {})",
            requested.display(),
            root.display()
        );
    }
    Ok(root)
}

#[cfg(test)]
pub fn load(path: &Path) -> Result<RepositoryData> {
    load_git_root(discover(path)?)
}

pub fn load_or_local(path: &Path) -> Result<RepositoryData> {
    match discover(path) {
        Ok(root) => load_git_root(root),
        Err(_) => local_workspace(path),
    }
}

fn load_git_root(root: PathBuf) -> Result<RepositoryData> {
    let (worktree, inventory, history, graph, refs) = thread::scope(|scope| {
        let worktree = scope.spawn(|| load_worktree(&root));
        let inventory = scope.spawn(|| load_git_inventory(&root));
        let history = scope.spawn(|| load_history(&root));
        let graph = scope.spawn(|| load_graph(&root));
        let refs = scope.spawn(|| load_refs(&root));

        Ok::<_, anyhow::Error>((
            worktree
                .join()
                .map_err(|_| anyhow!("status worker panicked"))??,
            inventory
                .join()
                .map_err(|_| anyhow!("file worker panicked"))??,
            history
                .join()
                .map_err(|_| anyhow!("history worker panicked"))??,
            graph
                .join()
                .map_err(|_| anyhow!("graph worker panicked"))??,
            refs.join().map_err(|_| anyhow!("refs worker panicked"))??,
        ))
    })?;

    Ok(RepositoryData {
        root,
        kind: RepositoryKind::Git,
        branch: history.branch,
        changes: worktree.changes,
        files: inventory.files,
        directories: inventory.directories,
        history: history.commits,
        commits: graph.commits,
        files_fingerprint: inventory.fingerprint,
        changes_fingerprint: worktree.fingerprint,
        change_counts: worktree.counts,
        graph_width: graph.width,
        graph_truncated: graph.truncated,
        branches: refs.branches,
        github_remote: refs.github_remote,
    })
}

pub fn refresh_repository(
    root: &Path,
    kind: RepositoryKind,
    scope: RefreshScope,
) -> Result<RepositoryUpdate> {
    if kind == RepositoryKind::Local {
        return Ok(RepositoryUpdate {
            root: root.to_owned(),
            worktree: None,
            inventory: scope
                .includes(RefreshScope::INVENTORY)
                .then(|| load_local_inventory(root))
                .transpose()?,
            history: None,
            graph: None,
            refs: None,
        });
    }

    let (worktree, inventory, history, graph, refs) = thread::scope(|thread_scope| {
        let worktree = scope
            .includes(RefreshScope::WORKTREE)
            .then(|| thread_scope.spawn(|| load_worktree(root)));
        let inventory = scope
            .includes(RefreshScope::INVENTORY)
            .then(|| thread_scope.spawn(|| load_git_inventory(root)));
        let history = scope
            .includes(RefreshScope::HISTORY)
            .then(|| thread_scope.spawn(|| load_history(root)));
        let graph = scope
            .includes(RefreshScope::GRAPH)
            .then(|| thread_scope.spawn(|| load_graph(root)));
        let refs = scope
            .includes(RefreshScope::REFS)
            .then(|| thread_scope.spawn(|| load_refs(root)));

        Ok::<_, anyhow::Error>((
            join_refresh_worker(worktree, "status")?,
            join_refresh_worker(inventory, "file")?,
            join_refresh_worker(history, "history")?,
            join_refresh_worker(graph, "graph")?,
            join_refresh_worker(refs, "refs")?,
        ))
    })?;

    Ok(RepositoryUpdate {
        root: root.to_owned(),
        worktree,
        inventory,
        history,
        graph,
        refs,
    })
}

fn join_refresh_worker<T>(
    worker: Option<thread::ScopedJoinHandle<'_, Result<T>>>,
    label: &str,
) -> Result<Option<T>> {
    worker
        .map(|worker| {
            worker
                .join()
                .map_err(|_| anyhow!("{label} worker panicked"))?
        })
        .transpose()
}

fn load_worktree(root: &Path) -> Result<WorktreeData> {
    let mut changes = status(root)?;
    populate_diff_stats(root, &mut changes)?;
    Ok(WorktreeData {
        fingerprint: fingerprint(&changes),
        counts: change_counts(&changes),
        changes,
    })
}

fn load_git_inventory(root: &Path) -> Result<InventoryData> {
    let (files, directories) = git_repository_entries(root)?;
    Ok(InventoryData {
        fingerprint: fingerprint(&(&files, &directories)),
        files,
        directories,
    })
}

fn load_local_inventory(root: &Path) -> Result<InventoryData> {
    let (files, directories) = local_entries(root)?;
    Ok(InventoryData {
        fingerprint: fingerprint(&(&files, &directories)),
        files,
        directories,
    })
}

fn load_history(root: &Path) -> Result<HistoryData> {
    let (branch, commits) = thread::scope(|scope| {
        let branch = scope.spawn(|| branch_name(root));
        let commits = scope.spawn(|| branch_history(root));
        Ok::<_, anyhow::Error>((
            branch
                .join()
                .map_err(|_| anyhow!("branch worker panicked"))??,
            commits
                .join()
                .map_err(|_| anyhow!("history worker panicked"))??,
        ))
    })?;
    Ok(HistoryData { branch, commits })
}

fn load_graph(root: &Path) -> Result<GraphData> {
    let (mut commits, truncated) = log(root)?;
    layout_graph(&mut commits);
    Ok(GraphData {
        width: graph_width(&commits),
        commits,
        truncated,
    })
}

fn load_refs(root: &Path) -> Result<RefsData> {
    let (branches, github_remote) = thread::scope(|scope| {
        let branches = scope.spawn(|| repository_branches(root));
        let github_remote = scope.spawn(|| repository_has_github_remote(root));
        Ok::<_, anyhow::Error>((
            branches
                .join()
                .map_err(|_| anyhow!("branch list worker panicked"))??,
            github_remote
                .join()
                .map_err(|_| anyhow!("remote worker panicked"))??,
        ))
    })?;
    Ok(RefsData {
        branches,
        github_remote,
    })
}

fn local_workspace(path: &Path) -> Result<RepositoryData> {
    let root = fs::canonicalize(path).context("could not resolve workspace directory")?;
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let inventory = load_local_inventory(&root)?;
    Ok(RepositoryData {
        root,
        kind: RepositoryKind::Local,
        branch: "local".to_owned(),
        changes: Vec::new(),
        files: inventory.files,
        directories: inventory.directories,
        history: Vec::new(),
        commits: Vec::new(),
        files_fingerprint: inventory.fingerprint,
        changes_fingerprint: fingerprint(&Vec::<Change>::new()),
        change_counts: (0, 0),
        graph_width: 0,
        graph_truncated: false,
        branches: Vec::new(),
        github_remote: false,
    })
}

fn repository_has_github_remote(root: &Path) -> Result<bool> {
    let output = run(root, &["remote", "-v"])?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(is_github_remote_url))
}

fn is_github_remote_url(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("git@github.com:")
        || value.starts_with("https://github.com/")
        || value.starts_with("http://github.com/")
        || value.starts_with("ssh://git@github.com/")
        || value.starts_with("git://github.com/")
}

fn repository_branches(root: &Path) -> Result<Vec<Branch>> {
    let output = run(
        root,
        &[
            "for-each-ref",
            "--format=%(HEAD)%1f%(refname)%1f%(refname:short)%1f%(objectname:short)%1f%(upstream:short)%1f%(committerdate:relative)%1f%(subject)%1e",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let mut branches = output
        .stdout
        .split(|byte| *byte == 0x1e)
        .filter_map(|record| {
            let record = trim_ascii(record);
            if record.is_empty() {
                return None;
            }
            let fields: Vec<_> = record.split(|byte| *byte == 0x1f).collect();
            if fields.len() != 7 {
                return None;
            }
            let text = |field: &[u8]| String::from_utf8_lossy(field).into_owned();
            let refname = text(fields[1]);
            let name = text(fields[2]);
            if refname.starts_with("refs/remotes/") && name.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name,
                upstream: text(fields[4]),
                oid: text(fields[3]),
                date: text(fields[5]),
                subject: text(fields[6]),
                remote: refname.starts_with("refs/remotes/"),
                current: fields[0] == b"*",
            })
        })
        .collect::<Vec<_>>();
    branches.sort_by(|left, right| {
        right
            .current
            .cmp(&left.current)
            .then_with(|| left.remote.cmp(&right.remote))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(branches)
}

fn git_repository_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let output = run(
        root,
        &[
            "ls-files",
            "-z",
            "-t",
            "--cached",
            "--others",
            "--exclude-standard",
            "--deleted",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let mut states = HashMap::<String, (bool, bool)>::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Some((&tag, path)) = entry.split_first() else {
            continue;
        };
        let path = path.strip_prefix(b" ").unwrap_or(path);
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path).into_owned();
        let absent_skip_worktree = tag == b'S' && root.join(&path).symlink_metadata().is_err();
        let state = states.entry(path).or_default();
        if tag == b'R' || absent_skip_worktree {
            state.1 = true;
        } else {
            state.0 = true;
        }
    }
    let mut files: Vec<String> = states
        .into_iter()
        .filter_map(|(path, (present, deleted))| (present && !deleted).then_some(path))
        .collect();
    let (ignored_files, ignored_directories) = ignored_repository_entries(root)?;
    files.extend(ignored_files);
    files.sort_unstable();
    files.dedup();
    let output = run(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--directory",
            "--empty-directory",
            "--exclude-standard",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let directory_roots: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| path.strip_suffix(b"/"))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .filter(|path| !path.is_empty())
        .collect();
    let mut directories = expand_git_directories(root, directory_roots)?;
    directories.extend(ignored_directories);
    let output = run(root, &["ls-files", "-z", "--stage"])?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let submodules: HashSet<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let separator = entry.iter().position(|byte| *byte == b'\t')?;
            let (metadata, path) = entry.split_at(separator);
            let path = path.get(1..)?;
            metadata
                .starts_with(b"160000 ")
                .then(|| String::from_utf8_lossy(path).into_owned())
        })
        .filter(|path| {
            root.join(path)
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir())
        })
        .collect();
    files.retain(|path| !submodules.contains(path));
    directories.extend(submodules);
    directories.sort_unstable();
    directories.dedup();
    Ok((files, directories))
}

fn ignored_repository_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let output = run(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    let output = run(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let directories = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| path.strip_suffix(b"/"))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .filter(|path| !path.is_empty())
        .collect();
    Ok((files, directories))
}

fn expand_git_directories(root: &Path, roots: Vec<String>) -> Result<Vec<String>> {
    let mut directories: HashSet<String> = roots.iter().cloned().collect();
    let mut frontier = roots;
    while !frontier.is_empty() {
        let mut candidates = Vec::new();
        for relative in frontier {
            let path = root.join(&relative);
            let Ok(entries) = fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_name() == ".git" {
                    continue;
                }
                let path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    continue;
                }
                if let Ok(path) = path.strip_prefix(root) {
                    let path = path.to_string_lossy();
                    candidates.push(if cfg!(windows) {
                        path.replace('\\', "/")
                    } else {
                        path.into_owned()
                    });
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let ignored = git_ignored_paths(root, &candidates)?;
        frontier = candidates
            .into_iter()
            .filter(|path| !ignored.contains(path))
            .filter(|path| directories.insert(path.clone()))
            .collect();
    }
    Ok(directories.into_iter().collect())
}

fn git_ignored_paths(root: &Path, paths: &[String]) -> Result<HashSet<String>> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut command = base_command(root);
    command
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("could not run git check-ignore")?;
    let input = paths.to_vec();
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || -> std::io::Result<()> {
            for path in input {
                stdin.write_all(path.as_bytes())?;
                stdin.write_all(&[0])?;
            }
            Ok(())
        })
    });
    let output = child
        .wait_with_output()
        .context("could not read git check-ignore")?;
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| anyhow!("git check-ignore input writer panicked"))?
            .context("could not write git check-ignore input")?;
    }
    if !output.status.success() && output.status.code() != Some(1) {
        bail!("{}", clean_stderr(&output));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

fn local_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let mut files = Vec::new();
    let mut directory_paths = Vec::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) if directory != root => continue,
            Err(error) => return Err(error).context("read repository files"),
        };
        for entry in entries.flatten() {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if let Ok(relative) = path.strip_prefix(root) {
                    directory_paths.push(relative.to_string_lossy().into_owned());
                }
                directories.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files.dedup();
    directory_paths.sort();
    directory_paths.dedup();
    Ok((files, directory_paths))
}

fn fingerprint<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn change_counts(changes: &[Change]) -> (usize, usize) {
    changes.iter().fold((0, 0), |(staged, unstaged), change| {
        if change.staged {
            (staged + 1, unstaged)
        } else {
            (staged, unstaged + 1)
        }
    })
}

fn graph_width(commits: &[Commit]) -> usize {
    commits
        .iter()
        .map(|commit| commit.graph.len())
        .max()
        .unwrap_or(1)
}

pub fn file_content(root: &Path, relative_path: &str) -> Result<String> {
    const MAX_PREVIEW_BYTES: u64 = 1_048_576;

    let path = root.join(relative_path);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(&path)
            .with_context(|| format!("could not read link {}", path.display()))?;
        return Ok(format!("Symbolic link -> {}", target.display()));
    }
    if metadata.is_dir() {
        return Ok("Directory\n\nThis path may be a Git submodule.".to_owned());
    }
    if metadata.len() > MAX_PREVIEW_BYTES {
        return Ok(format!(
            "File is too large to preview\n\n{} bytes",
            metadata.len()
        ));
    }
    let bytes = fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
    if bytes.contains(&0) {
        return Ok(format!("Binary file\n\n{} bytes", bytes.len()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn stage(root: &Path, change: &Change) -> Result<()> {
    let mut args = vec!["add", "--"];
    if let Some(original) = &change.original_path {
        args.push(original);
    }
    args.push(&change.path);
    run_ok(root, &args)
}

pub fn unstage(root: &Path, change: &Change) -> Result<()> {
    let mut args = vec!["restore", "--staged", "--"];
    if let Some(original) = &change.original_path {
        args.push(original);
    }
    args.push(&change.path);
    let output = run(root, &args)?;
    if output.status.success() {
        return Ok(());
    }

    // `restore --staged` cannot address an unborn HEAD, while reset can.
    let mut args = vec!["reset", "--"];
    if let Some(original) = &change.original_path {
        args.push(original);
    }
    args.push(&change.path);
    run_ok(root, &args)
}

pub fn stage_all(root: &Path) -> Result<()> {
    run_ok(root, &["add", "-A"])
}

pub fn unstage_all(root: &Path) -> Result<()> {
    let output = run(root, &["restore", "--staged", "."])?;
    if output.status.success() {
        return Ok(());
    }
    run_ok(root, &["reset"])
}

pub fn stage_hunk(root: &Path, diff: &str, index: usize) -> Result<()> {
    let patch = hunk_patch(diff, index).context("diff hunk is no longer available")?;
    let mut child = base_command(root)
        .args(["apply", "--cached", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("could not start git apply --cached")?;
    child
        .stdin
        .take()
        .context("could not open git apply input")?
        .write_all(patch.as_bytes())
        .context("could not write diff hunk to git apply")?;
    let output = child
        .wait_with_output()
        .context("could not finish git apply --cached")?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(())
}

fn hunk_patch(diff: &str, target: usize) -> Option<String> {
    let lines: Vec<&str> = diff.lines().collect();
    let first_hunk = lines.iter().position(|line| line.starts_with("@@"))?;
    let start = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("@@"))
        .nth(target)?
        .0;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("@@") || line.starts_with("diff --git"))
        .map_or(lines.len(), |offset| start + 1 + offset);

    let mut patch = lines[..first_hunk].join("\n");
    patch.push('\n');
    patch.push_str(&lines[start..end].join("\n"));
    patch.push('\n');
    Some(patch)
}

pub fn commit(root: &Path, message: &str) -> Result<CommandOutput> {
    if message.trim().is_empty() {
        bail!("Commit message cannot be empty");
    }
    let output = run(root, &["commit", "-m", message.trim()])?;
    Ok(command_output(output))
}

pub(crate) fn commit_draft_path(root: &Path) -> Result<PathBuf> {
    let output = run(root, &["rev-parse", "--absolute-git-dir"])?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let git_directory = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if git_directory.is_empty() {
        bail!("Git returned an empty repository directory");
    }
    Ok(PathBuf::from(git_directory).join("HUNKLE_COMMIT_DRAFT"))
}

pub fn fetch(root: &Path) -> Result<CommandOutput> {
    let output = run(root, &["fetch", "--all", "--prune"])?;
    Ok(command_output(output))
}

pub fn run_command(root: &Path, args: &[String]) -> Result<CommandOutput> {
    let output = base_command(root)
        .args(args)
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))?;
    Ok(command_output(output))
}

pub fn worktree_signature(root: &Path) -> Result<u64> {
    let output = run(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }

    let mut signature = DefaultHasher::new();
    output.stdout.hash(&mut signature);
    for record in output.stdout.split(|byte| *byte == 0) {
        let Some(path) = porcelain_v2_path(record) else {
            continue;
        };
        path.hash(&mut signature);
        let path = root.join(String::from_utf8_lossy(path).as_ref());
        if let Ok(metadata) = fs::symlink_metadata(path) {
            metadata.len().hash(&mut signature);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(&mut signature);
        }
    }
    Ok(signature.finish())
}

fn porcelain_v2_path(record: &[u8]) -> Option<&[u8]> {
    match record.first()? {
        b'1' => record.splitn(9, |byte| *byte == b' ').nth(8),
        b'2' => record.splitn(10, |byte| *byte == b' ').nth(9),
        b'u' => record.splitn(11, |byte| *byte == b' ').nth(10),
        b'?' => record.strip_prefix(b"? "),
        _ => None,
    }
}

pub fn diff(root: &Path, change: &Change) -> Result<String> {
    if !change.staged && change.code == '?' {
        const MAX_UNTRACKED_PREVIEW_BYTES: u64 = 128 * 1024;
        const MAX_UNTRACKED_PREVIEW_LINES: usize = 500;

        let path = root.join(&change.path);
        let metadata =
            fs::metadata(&path).with_context(|| format!("could not inspect {}", path.display()))?;
        let mut bytes =
            Vec::with_capacity(metadata.len().min(MAX_UNTRACKED_PREVIEW_BYTES + 1) as usize);
        fs::File::open(&path)
            .with_context(|| format!("could not read {}", path.display()))?
            .take(MAX_UNTRACKED_PREVIEW_BYTES + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("could not read {}", path.display()))?;
        if bytes.contains(&0) {
            return Ok(format!("Binary untracked file\n\n{} bytes", metadata.len()));
        }
        let byte_truncated = bytes.len() > MAX_UNTRACKED_PREVIEW_BYTES as usize;
        bytes.truncate(MAX_UNTRACKED_PREVIEW_BYTES as usize);
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        let preview = lines
            .by_ref()
            .take(MAX_UNTRACKED_PREVIEW_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        let line_truncated = lines.next().is_some();
        let suffix = if byte_truncated || line_truncated {
            format!("\n\n[Preview truncated; file is {} bytes]", metadata.len())
        } else {
            String::new()
        };
        return Ok(format!(
            "Untracked file: {}\n\n{preview}{suffix}",
            change.path
        ));
    }

    let mut args = if change.staged {
        vec!["diff", "--cached", "--no-ext-diff", "--unified=3", "--"]
    } else {
        vec!["diff", "--no-ext-diff", "--unified=3", "--"]
    };
    if let Some(original) = &change.original_path {
        args.push(original);
    }
    args.push(&change.path);
    let output = run(root, &args)?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn commit_diff(root: &Path, oid: &str) -> Result<String> {
    let output = run(
        root,
        &[
            "show",
            "--format=",
            "--no-ext-diff",
            "--first-parent",
            "--unified=3",
            oid,
        ],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn commit_summaries(root: &Path, oids: &[String]) -> Result<HashMap<String, DiffSummary>> {
    if oids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut args = vec![
        "show",
        "--numstat",
        "-z",
        "--format=%x1e%H%x00",
        "--no-renames",
        "--first-parent",
    ];
    args.extend(oids.iter().map(String::as_str));
    let output = run(root, &args)?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(parse_commit_summaries(&output.stdout))
}

fn parse_commit_summaries(bytes: &[u8]) -> HashMap<String, DiffSummary> {
    let mut summaries = HashMap::new();
    for record in bytes.split(|byte| *byte == 0x1e) {
        let Some(separator) = record.iter().position(|byte| *byte == 0) else {
            continue;
        };
        let (oid, entries) = record.split_at(separator);
        let entries = &entries[1..];
        let oid = String::from_utf8_lossy(trim_ascii(oid)).into_owned();
        if oid.is_empty() {
            continue;
        }
        let mut summary = DiffSummary::default();
        for entry in entries.split(|byte| *byte == 0) {
            let entry = entry.strip_prefix(b"\n").unwrap_or(entry);
            if entry.is_empty() {
                continue;
            }
            let mut fields = entry.splitn(3, |byte| *byte == b'\t');
            let (Some(additions), Some(deletions), Some(path)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            summary.additions = summary
                .additions
                .saturating_add(String::from_utf8_lossy(additions).parse().unwrap_or(0));
            summary.deletions = summary
                .deletions
                .saturating_add(String::from_utf8_lossy(deletions).parse().unwrap_or(0));
            summary
                .files
                .push(String::from_utf8_lossy(path).into_owned());
        }
        summaries.insert(oid, summary);
    }
    summaries
}

fn branch_name(root: &Path) -> Result<String> {
    let output = run(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }

    let output = run(root, &["rev-parse", "--short", "HEAD"])?;
    if output.status.success() {
        Ok(format!(
            "detached @ {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    } else {
        Ok("no commits".to_owned())
    }
}

fn status(root: &Path) -> Result<Vec<Change>> {
    let output = run(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(parse_status(&output.stdout))
}

fn parse_status(bytes: &[u8]) -> Vec<Change> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut changes = Vec::new();
    let mut index = 0;

    while index < fields.len() {
        let field = fields[index];
        if field.len() < 4 {
            index += 1;
            continue;
        }

        let x = field[0] as char;
        let y = field[1] as char;
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        let renamed = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
        let original_path = renamed
            .then(|| fields.get(index + 1))
            .flatten()
            .map(|path| String::from_utf8_lossy(path).into_owned());

        if x != ' ' && x != '?' && x != '!' {
            changes.push(Change {
                path: path.clone(),
                original_path: original_path.clone(),
                code: x,
                staged: true,
                additions: 0,
                deletions: 0,
            });
        }
        if y != ' ' && y != '!' {
            changes.push(Change {
                path,
                original_path,
                code: y,
                staged: false,
                additions: 0,
                deletions: 0,
            });
        }

        if renamed {
            index += 1;
        }
        index += 1;
    }

    changes.sort_by(|a, b| b.staged.cmp(&a.staged).then_with(|| a.path.cmp(&b.path)));
    changes
}

fn populate_diff_stats(root: &Path, changes: &mut [Change]) -> Result<()> {
    let staged = diff_stats(root, true)?;
    let unstaged = diff_stats(root, false)?;
    for change in changes {
        if change.code == '?' && !change.staged {
            change.additions = count_file_lines(&root.join(&change.path)).unwrap_or(0);
            continue;
        }
        let stats = if change.staged { &staged } else { &unstaged };
        let (mut additions, mut deletions) = stats.get(&change.path).copied().unwrap_or_default();
        if let Some(original) = &change.original_path {
            let original_stats = stats.get(original).copied().unwrap_or_default();
            additions = additions.saturating_add(original_stats.0);
            deletions = deletions.saturating_add(original_stats.1);
        }
        change.additions = additions;
        change.deletions = deletions;
    }
    Ok(())
}

fn diff_stats(root: &Path, staged: bool) -> Result<HashMap<String, (u64, u64)>> {
    let args = if staged {
        ["diff", "--cached", "--no-renames", "--numstat", "-z"].as_slice()
    } else {
        ["diff", "--no-renames", "--numstat", "-z"].as_slice()
    };
    let output = run(root, args)?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    let mut stats = HashMap::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let additions = String::from_utf8_lossy(additions).parse().unwrap_or(0);
        let deletions = String::from_utf8_lossy(deletions).parse().unwrap_or(0);
        stats.insert(
            String::from_utf8_lossy(path).into_owned(),
            (additions, deletions),
        );
    }
    Ok(stats)
}

fn count_file_lines(path: &Path) -> Result<u64> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut lines = 0u64;
    let mut has_bytes = false;
    let mut ends_with_newline = true;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        if buffer.contains(&0) {
            return Ok(0);
        }
        has_bytes = true;
        lines = lines.saturating_add(buffer.iter().filter(|byte| **byte == b'\n').count() as u64);
        ends_with_newline = buffer.last() == Some(&b'\n');
        let consumed = buffer.len();
        reader.consume(consumed);
    }
    Ok(lines + u64::from(has_bytes && !ends_with_newline))
}

const GRAPH_COMMIT_LIMIT: usize = 5_000;

fn log(root: &Path) -> Result<(Vec<Commit>, bool)> {
    let commits = read_log(
        root,
        &[
            "--date-order",
            "--ignore-missing",
            "--max-count=5001",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
        ],
    )?;
    Ok(cap_graph_commits(commits))
}

fn cap_graph_commits(mut commits: Vec<Commit>) -> (Vec<Commit>, bool) {
    let truncated = commits.len() > GRAPH_COMMIT_LIMIT;
    commits.truncate(GRAPH_COMMIT_LIMIT);
    (commits, truncated)
}

fn branch_history(root: &Path) -> Result<Vec<Commit>> {
    read_log(
        root,
        &[
            "--date-order",
            "--ignore-missing",
            "--max-count=200",
            "HEAD",
        ],
    )
}

fn read_log(root: &Path, revisions: &[&str]) -> Result<Vec<Commit>> {
    let format = "--format=%H%x1f%P%x1f%D%x1f%an%x1f%ad%x1f%s%x1e";
    let mut args = vec![
        "log",
        format,
        "--date=format:%Y-%m-%d %H:%M",
        "--decorate=short",
    ];
    args.extend_from_slice(revisions);
    let output = run(root, &args)?;

    if !output.status.success() {
        let stderr = clean_stderr(&output);
        if stderr.contains("does not have any commits yet")
            || stderr.contains("bad revision 'HEAD'")
            || stderr.contains("ambiguous argument 'HEAD'")
        {
            return Ok(Vec::new());
        }
        bail!("{stderr}");
    }

    Ok(parse_log(&output.stdout))
}

fn parse_log(bytes: &[u8]) -> Vec<Commit> {
    bytes
        .split(|byte| *byte == 0x1e)
        .filter_map(|record| {
            let record = trim_ascii(record);
            if record.is_empty() {
                return None;
            }
            let fields: Vec<&[u8]> = record.split(|byte| *byte == 0x1f).collect();
            if fields.len() != 6 {
                return None;
            }
            let text = |field: &[u8]| String::from_utf8_lossy(field).into_owned();
            let decorations = text(fields[2]);
            Some(Commit {
                oid: text(fields[0]),
                parents: text(fields[1])
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                refs: decorations
                    .split(", ")
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
                author: text(fields[3]),
                date: text(fields[4]),
                subject: text(fields[5]),
                graph: Vec::new(),
            })
        })
        .collect()
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn layout_graph(commits: &mut [Commit]) {
    let mut oid_ids = HashMap::new();
    let mut next_oid = 0usize;
    for commit in commits.iter() {
        for oid in std::iter::once(&commit.oid).chain(commit.parents.iter()) {
            oid_ids.entry(oid.clone()).or_insert_with(|| {
                let id = next_oid;
                next_oid += 1;
                id
            });
        }
    }

    let mut lanes: Vec<Option<usize>> = Vec::new();
    let mut colors: Vec<usize> = Vec::new();
    let mut next_color = 0;

    for commit in commits {
        let commit_id = oid_ids[&commit.oid];
        let incoming: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, oid)| (*oid == Some(commit_id)).then_some(index))
            .collect();

        let node = incoming.first().copied().unwrap_or_else(|| {
            if let Some(index) = lanes.iter().position(Option::is_none) {
                lanes[index] = Some(commit_id);
                colors[index] = next_color;
                next_color += 1;
                index
            } else {
                lanes.push(Some(commit_id));
                colors.push(next_color);
                next_color += 1;
                lanes.len() - 1
            }
        });

        let before_len = lanes.len();
        let mut after = lanes.clone();
        for lane in incoming.iter().copied().skip(1) {
            after[lane] = None;
        }

        if let Some(first_parent) = commit.parents.first() {
            after[node] = Some(oid_ids[first_parent]);
        } else {
            after[node] = None;
        }

        let mut outgoing = Vec::new();
        for parent in commit.parents.iter().skip(1) {
            let parent_id = oid_ids[parent];
            let destination = after
                .iter()
                .position(|oid| *oid == Some(parent_id))
                .unwrap_or_else(|| {
                    if let Some(index) = after.iter().position(Option::is_none) {
                        after[index] = Some(parent_id);
                        colors[index] = next_color;
                        next_color += 1;
                        index
                    } else {
                        after.push(Some(parent_id));
                        colors.push(next_color);
                        next_color += 1;
                        after.len() - 1
                    }
                });
            outgoing.push(destination);
        }

        let lane_count = before_len.max(after.len()).max(node + 1);
        let mut masks = vec![0_u8; lane_count.saturating_mul(2).saturating_sub(1)];
        let mut cell_colors = vec![colors.get(node).copied().unwrap_or(0); masks.len()];

        for (index, lane) in lanes.iter().enumerate() {
            if lane.is_some() {
                masks[index * 2] |= UP;
                cell_colors[index * 2] = colors[index];
            }
        }
        for (index, lane) in after.iter().enumerate() {
            if lane.is_some() {
                masks[index * 2] |= DOWN;
                cell_colors[index * 2] = colors[index];
            }
        }

        for destination in incoming.iter().copied().skip(1).chain(outgoing) {
            connect(
                &mut masks,
                &mut cell_colors,
                node * 2,
                destination * 2,
                colors[node],
            );
        }

        commit.graph = masks
            .into_iter()
            .enumerate()
            .map(|(index, mask)| GraphCell {
                symbol: if index == node * 2 {
                    '●'
                } else {
                    glyph(mask)
                },
                color: cell_colors[index],
            })
            .collect();

        lanes = after;
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
            colors.pop();
        }
    }
}

fn connect(masks: &mut [u8], colors: &mut [usize], from: usize, to: usize, color: usize) {
    let (left, right) = if from <= to { (from, to) } else { (to, from) };
    for index in left..=right {
        if index > left {
            masks[index] |= LEFT;
        }
        if index < right {
            masks[index] |= RIGHT;
        }
        colors[index] = color;
    }
}

fn glyph(mask: u8) -> char {
    match mask {
        0 => ' ',
        3 => '│',
        12 => '─',
        10 => '╭',
        6 => '╮',
        9 => '╰',
        5 => '╯',
        11 => '├',
        7 => '┤',
        14 => '┬',
        13 => '┴',
        15 => '┼',
        UP => '╵',
        DOWN => '╷',
        LEFT => '╴',
        RIGHT => '╶',
        _ => '┼',
    }
}

fn run(root: &Path, args: &[&str]) -> Result<Output> {
    base_command(root)
        .args(args)
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))
}

fn base_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["--no-pager", "--no-optional-locks"])
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env("GIT_EDITOR", "false")
        .env("GIT_SEQUENCE_EDITOR", "false")
        .stdin(Stdio::null());
    command
}

fn run_ok(root: &Path, args: &[&str]) -> Result<()> {
    let output = run(root, args)?;
    if !output.status.success() {
        bail!("{}", clean_stderr(&output));
    }
    Ok(())
}

fn clean_stderr(output: &Output) -> String {
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if message.is_empty() {
        format!("Git exited with {}", output.status)
    } else {
        message
    }
}

fn command_output(output: Output) -> CommandOutput {
    CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
        exit_code: output.status.code(),
    }
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_staged_and_unstaged_status_entries() {
        let parsed = parse_status(b"M  staged.rs\0 M changed.rs\0?? new.rs\0MM both.rs\0");
        assert_eq!(parsed.len(), 5);
        assert!(
            parsed
                .iter()
                .any(|change| change.path == "staged.rs" && change.staged)
        );
        assert!(
            parsed
                .iter()
                .any(|change| change.path == "new.rs" && !change.staged)
        );
        assert_eq!(
            parsed
                .iter()
                .filter(|change| change.path == "both.rs")
                .count(),
            2
        );
    }

    #[test]
    fn preserves_both_paths_for_renames() {
        let parsed = parse_status(b"R  new.rs\0old.rs\0");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "new.rs");
        assert_eq!(parsed[0].original_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn does_not_climb_to_an_enclosing_repository() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        let nested = root.join("nested/config");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("settings.toml"), "theme = 'test'\n").unwrap();

        assert_eq!(discover(root).unwrap(), fs::canonicalize(root).unwrap());
        let error = discover(&nested).unwrap_err().to_string();
        assert!(error.contains("not a repository root"));
        assert!(error.contains(&fs::canonicalize(root).unwrap().display().to_string()));
        assert!(load(&nested).is_err());

        let workspace = load_or_local(&nested).unwrap();
        assert_eq!(workspace.kind, RepositoryKind::Local);
        assert_eq!(workspace.root, fs::canonicalize(&nested).unwrap());
        assert_eq!(workspace.branch, "local");
        assert_eq!(workspace.files, ["settings.toml"]);
        assert!(workspace.changes.is_empty());
        assert!(workspace.history.is_empty());
        assert!(workspace.commits.is_empty());
    }

    #[test]
    fn loads_a_plain_directory_as_a_local_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(root.join("README.md"), "local\n").unwrap();
        fs::write(root.join("src/components/card.rs"), "component\n").unwrap();

        let workspace = load_or_local(root).unwrap();
        assert_eq!(workspace.kind, RepositoryKind::Local);
        assert_eq!(workspace.root, fs::canonicalize(root).unwrap());
        assert_eq!(workspace.branch, "local");
        assert_eq!(workspace.files, ["README.md", "src/components/card.rs"]);
        assert!(workspace.changes.is_empty());
        assert!(workspace.history.is_empty());
        assert!(workspace.commits.is_empty());
    }

    #[test]
    fn lays_out_a_merge_as_connected_cells() {
        let mut commits = vec![
            commit("merge", &["left", "right"]),
            commit("left", &["base"]),
            commit("right", &["base"]),
            commit("base", &[]),
        ];
        layout_graph(&mut commits);
        assert!(commits.iter().all(|commit| {
            commit.graph.contains(&GraphCell {
                symbol: '●',
                color: commit
                    .graph
                    .iter()
                    .find(|cell| cell.symbol == '●')
                    .unwrap()
                    .color,
            })
        }));
        assert!(
            commits[0]
                .graph
                .iter()
                .any(|cell| matches!(cell.symbol, '─' | '╮' | '╭'))
        );
        assert_eq!(
            commits
                .iter()
                .map(|commit| commit
                    .graph
                    .iter()
                    .map(|cell| cell.symbol)
                    .collect::<String>())
                .collect::<Vec<_>>(),
            ["●─╮", "● │", "│ ●", "●─╯"]
        );
    }

    #[test]
    fn lays_out_linear_history_without_extra_lanes() {
        let mut commits = vec![
            commit("three", &["two"]),
            commit("two", &["one"]),
            commit("one", &[]),
        ];
        layout_graph(&mut commits);
        assert_eq!(
            commits
                .iter()
                .map(|commit| commit
                    .graph
                    .iter()
                    .map(|cell| cell.symbol)
                    .collect::<String>())
                .collect::<Vec<_>>(),
            ["●", "●", "●"]
        );
    }

    #[test]
    fn lays_out_a_distinct_branch_exactly() {
        let mut commits = vec![
            commit("main", &["base"]),
            commit("side", &["base"]),
            commit("base", &[]),
        ];
        layout_graph(&mut commits);
        let symbols = commits
            .iter()
            .map(|commit| {
                commit
                    .graph
                    .iter()
                    .map(|cell| cell.symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(symbols, ["●", "│ ●", "●─╯"]);
    }

    #[test]
    fn caps_graph_commits_and_reports_truncation() {
        let commits = (0..=GRAPH_COMMIT_LIMIT)
            .map(|index| commit(&index.to_string(), &[]))
            .collect();
        let (commits, truncated) = cap_graph_commits(commits);
        assert_eq!(commits.len(), GRAPH_COMMIT_LIMIT);
        assert!(truncated);
        let mut commits = commits;
        layout_graph(&mut commits);
        assert!(
            commits
                .iter()
                .all(|commit| { commit.graph.len() == 1 && commit.graph[0].symbol == '●' })
        );
    }

    #[test]
    fn recognizes_github_remote_urls() {
        assert!(is_github_remote_url("git@github.com:owner/repo.git"));
        assert!(is_github_remote_url("https://github.com/owner/repo.git"));
        assert!(is_github_remote_url("ssh://git@github.com/owner/repo.git"));
        assert!(!is_github_remote_url("https://gitlab.com/owner/repo.git"));
    }

    fn commit(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.to_owned(),
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
            refs: Vec::new(),
            author: "A".to_owned(),
            date: "2026-01-01".to_owned(),
            subject: oid.to_owned(),
            graph: Vec::new(),
        }
    }

    #[test]
    fn loads_a_real_repository_with_a_merge_and_worktree_change() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Test Author"]);
        git(root, &["config", "user.email", "test@example.com"]);
        fs::write(root.join("base.txt"), "base\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "base"]);
        git(root, &["checkout", "-b", "feature"]);
        fs::write(root.join("feature.txt"), "feature\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "feature"]);
        git(root, &["checkout", "main"]);
        fs::write(root.join("main.txt"), "main\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "main work"]);
        git(
            root,
            &["merge", "--no-ff", "feature", "-m", "merge feature"],
        );
        fs::write(root.join("main.txt"), "changed\n").unwrap();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/cache.txt"), "generated\n").unwrap();

        let repo = load(root).unwrap();
        assert_eq!(repo.branch, "main");
        assert_eq!(
            repo.branches
                .iter()
                .map(|branch| (branch.name.as_str(), branch.current))
                .collect::<Vec<_>>(),
            [("main", true), ("feature", false)]
        );
        assert_eq!(repo.commits.len(), 4);
        assert_eq!(repo.history.len(), 4);
        assert_eq!(repo.commits[0].date.len(), 16);
        assert_eq!(repo.commits[0].date.as_bytes().get(10), Some(&b' '));
        assert_eq!(repo.commits[0].date.as_bytes().get(13), Some(&b':'));
        assert!(
            repo.history[0]
                .refs
                .iter()
                .any(|name| name.contains("HEAD"))
        );
        assert_eq!(repo.commits[0].parents.len(), 2);
        assert!(repo.commits[0].graph.iter().any(|cell| cell.symbol == '─'));
        assert_eq!(repo.changes.len(), 1);
        assert_eq!(repo.changes[0].path, "main.txt");
        assert_eq!(
            (repo.changes[0].additions, repo.changes[0].deletions),
            (1, 1)
        );
        assert!(repo.files.iter().any(|path| path == "base.txt"));
        assert!(repo.files.iter().any(|path| path == "feature.txt"));
        assert!(repo.files.iter().any(|path| path == "ignored/cache.txt"));
        assert!(
            !repo
                .changes
                .iter()
                .any(|change| change.path == "ignored/cache.txt")
        );
        assert_eq!(file_content(root, "main.txt").unwrap(), "changed\n");
        let selected_commit_diff = commit_diff(root, &repo.history[0].oid).unwrap();
        assert!(selected_commit_diff.contains("diff --git"));

        stage(root, &repo.changes[0]).unwrap();
        let staged = load(root).unwrap();
        assert!(staged.changes[0].staged);
        assert_eq!(
            (staged.changes[0].additions, staged.changes[0].deletions),
            (1, 1)
        );

        unstage(root, &staged.changes[0]).unwrap();
        let unstaged = load(root).unwrap();
        assert!(!unstaged.changes[0].staged);

        stage(root, &unstaged.changes[0]).unwrap();
        let output = super::commit(root, "update main").unwrap();
        assert!(output.success, "{}", output.stderr);
        let committed = load(root).unwrap();
        assert!(committed.changes.is_empty());
        assert_eq!(committed.commits.len(), 5);
        assert_eq!(committed.history.len(), 5);

        let fetched = super::fetch(root).unwrap();
        assert!(fetched.success, "{}", fetched.stderr);
    }

    #[test]
    fn git_files_include_untracked_and_ignored_files_but_exclude_deleted_tracked_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        git(root, &["add", "tracked.txt"]);
        fs::write(root.join("untracked.txt"), "new\n").unwrap();
        fs::create_dir_all(root.join("empty/nested")).unwrap();
        fs::create_dir_all(root.join("empty/ignored")).unwrap();
        fs::create_dir(root.join("config")).unwrap();
        fs::write(root.join(".gitignore"), "empty/ignored/\n.env*\nconfig/\n").unwrap();
        fs::write(root.join(".env"), "SECRET=value\n").unwrap();
        fs::write(root.join(".env.local"), "SECRET=local\n").unwrap();
        fs::write(root.join(".envrc"), "not an env file\n").unwrap();
        fs::write(root.join("config/.env.production"), "SECRET=prod\n").unwrap();
        fs::remove_file(root.join("tracked.txt")).unwrap();

        let (files, directories) = git_repository_entries(root).unwrap();
        assert_eq!(
            files,
            [
                ".env",
                ".env.local",
                ".envrc",
                ".gitignore",
                "config/.env.production",
                "untracked.txt"
            ]
        );
        assert_eq!(
            directories,
            ["config", "empty", "empty/ignored", "empty/nested"]
        );
    }

    #[test]
    fn gitlinks_are_exposed_as_directories() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "user.email", "test@example.com"]);
        fs::write(root.join("tracked"), "content").unwrap();
        git(root, &["add", "tracked"]);
        git(root, &["commit", "-m", "initial"]);
        let oid_output = run(root, &["rev-parse", "HEAD"]).unwrap();
        let oid = String::from_utf8_lossy(&oid_output.stdout);
        let cache_info = format!("160000,{},{path}", oid.trim(), path = "module");
        git(root, &["update-index", "--add", "--cacheinfo", &cache_info]);
        fs::create_dir(root.join("module")).unwrap();

        let (files, directories) = git_repository_entries(root).unwrap();
        assert!(!files.iter().any(|path| path == "module"));
        assert!(directories.iter().any(|path| path == "module"));
    }

    #[test]
    fn git_files_exclude_absent_sparse_checkout_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        fs::write(root.join("sparse.txt"), "tracked\n").unwrap();
        git(root, &["add", "sparse.txt"]);
        git(root, &["update-index", "--skip-worktree", "sparse.txt"]);
        fs::remove_file(root.join("sparse.txt")).unwrap();

        assert!(git_repository_entries(root).unwrap().0.is_empty());
    }

    #[test]
    fn truncates_large_untracked_previews() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("large.txt"), vec![b'x'; 256 * 1024]).unwrap();
        let change = Change {
            path: "large.txt".to_owned(),
            original_path: None,
            code: '?',
            staged: false,
            additions: 0,
            deletions: 0,
        };

        let preview = diff(root, &change).unwrap();
        assert!(preview.contains("Preview truncated; file is 262144 bytes"));
        assert!(preview.len() < 140 * 1024);
    }

    #[test]
    fn scoped_refresh_updates_only_requested_facets() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Test Author"]);
        git(root, &["config", "user.email", "test@example.com"]);
        fs::write(root.join("tracked.txt"), "base\n").unwrap();
        git(root, &["add", "tracked.txt"]);
        git(root, &["commit", "-m", "base"]);

        let mut repository = load(root).unwrap();
        let original_commit = repository.commits[0].oid.clone();
        let original_files = repository.files.clone();
        fs::write(root.join("tracked.txt"), "changed\n").unwrap();
        fs::write(root.join("new.txt"), "new\n").unwrap();

        let update = refresh_repository(root, RepositoryKind::Git, RefreshScope::WORKTREE).unwrap();
        assert!(update.worktree.is_some());
        assert!(update.inventory.is_none());
        assert!(update.history.is_none());
        assert!(update.graph.is_none());
        assert!(update.refs.is_none());
        repository.apply(update);
        assert_eq!(repository.files, original_files);
        assert_eq!(repository.commits[0].oid, original_commit);
        assert!(
            repository
                .changes
                .iter()
                .any(|change| change.path == "tracked.txt")
        );

        let update = refresh_repository(
            root,
            RepositoryKind::Git,
            RefreshScope::WORKTREE_AND_INVENTORY,
        )
        .unwrap();
        assert!(update.worktree.is_some());
        assert!(update.inventory.is_some());
        repository.apply(update);
        assert!(repository.files.iter().any(|file| file == "new.txt"));
        assert_eq!(repository.commits[0].oid, original_commit);
    }

    #[test]
    fn parses_batched_commit_change_summaries() {
        let summaries = parse_commit_summaries(
            b"\x1eabc123\0\0\n12\t3\tsrc/app.rs\0-\t-\tassets/logo.png\0\x1edef456\0\0\n4\t0\tREADME.md\0",
        );

        assert_eq!(
            summaries["abc123"],
            DiffSummary {
                files: vec!["src/app.rs".to_owned(), "assets/logo.png".to_owned()],
                additions: 12,
                deletions: 3,
            }
        );
        assert_eq!(
            summaries["def456"],
            DiffSummary {
                files: vec!["README.md".to_owned()],
                additions: 4,
                deletions: 0,
            }
        );
    }

    #[test]
    fn stages_only_the_selected_hunk() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Test Author"]);
        git(root, &["config", "user.email", "test@example.com"]);
        let original = (1..=20)
            .map(|line| format!("line {line:02}"))
            .collect::<Vec<_>>();
        fs::write(root.join("split.txt"), original.join("\n") + "\n").unwrap();
        git(root, &["add", "split.txt"]);
        git(root, &["commit", "-m", "base"]);

        let mut changed = original;
        changed[1] = "changed first".to_owned();
        changed[18] = "changed second".to_owned();
        fs::write(root.join("split.txt"), changed.join("\n") + "\n").unwrap();
        let change = load(root).unwrap().changes.remove(0);
        let patch = diff(root, &change).unwrap();
        assert_eq!(
            patch.lines().filter(|line| line.starts_with("@@")).count(),
            2
        );

        stage_hunk(root, &patch, 0).unwrap();

        let staged = String::from_utf8(
            run(root, &["diff", "--cached", "--", "split.txt"])
                .unwrap()
                .stdout,
        )
        .unwrap();
        let unstaged =
            String::from_utf8(run(root, &["diff", "--", "split.txt"]).unwrap().stdout).unwrap();
        assert!(staged.contains("changed first"));
        assert!(!staged.contains("changed second"));
        assert!(!unstaged.contains("changed first"));
        assert!(unstaged.contains("changed second"));
        let changes = load(root).unwrap().changes;
        assert!(
            changes
                .iter()
                .any(|change| change.path == "split.txt" && change.staged)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.path == "split.txt" && !change.staged)
        );
    }

    #[cfg(test)]
    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
