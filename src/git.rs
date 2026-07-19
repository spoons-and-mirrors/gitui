use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone)]
pub struct RepositoryData {
    pub root: PathBuf,
    pub branch: String,
    pub changes: Vec<Change>,
    pub files: Vec<String>,
    pub history: Vec<Commit>,
    pub commits: Vec<Commit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Ok(PathBuf::from(root))
}

pub fn load(path: &Path) -> Result<RepositoryData> {
    let root = discover(path)?;
    let (branch, changes, files, history, commits) = thread::scope(|scope| {
        let branch = scope.spawn(|| branch_name(&root));
        let changes = scope.spawn(|| -> Result<Vec<Change>> {
            let mut changes = status(&root)?;
            populate_diff_stats(&root, &mut changes)?;
            Ok(changes)
        });
        let files = scope.spawn(|| repository_files(&root));
        let history = scope.spawn(|| branch_history(&root));
        let commits = scope.spawn(|| -> Result<Vec<Commit>> {
            let mut commits = log(&root)?;
            layout_graph(&mut commits);
            Ok(commits)
        });

        Ok::<_, anyhow::Error>((
            branch
                .join()
                .map_err(|_| anyhow!("branch worker panicked"))??,
            changes
                .join()
                .map_err(|_| anyhow!("status worker panicked"))??,
            files
                .join()
                .map_err(|_| anyhow!("file worker panicked"))??,
            history
                .join()
                .map_err(|_| anyhow!("history worker panicked"))??,
            commits
                .join()
                .map_err(|_| anyhow!("graph worker panicked"))??,
        ))
    })?;

    Ok(RepositoryData {
        root,
        branch,
        changes,
        files,
        history,
        commits,
    })
}

pub fn repository_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
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
                directories.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
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

pub fn commit(root: &Path, message: &str) -> Result<CommandOutput> {
    if message.trim().is_empty() {
        bail!("Commit message cannot be empty");
    }
    let output = run(root, &["commit", "-m", message.trim()])?;
    Ok(command_output(output))
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
            "-c",
            "core.fsmonitor=false",
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
        let path = root.join(&change.path);
        let bytes =
            fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        if bytes.contains(&0) {
            return Ok(format!("Binary untracked file\n\n{} bytes", bytes.len()));
        }
        let text = String::from_utf8_lossy(&bytes);
        let preview: String = text.lines().take(500).collect::<Vec<_>>().join("\n");
        return Ok(format!("Untracked file: {}\n\n{preview}", change.path));
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

fn log(root: &Path) -> Result<Vec<Commit>> {
    read_log(
        root,
        &[
            "--date-order",
            "--ignore-missing",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
        ],
    )
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
    let mut args = vec!["log", format, "--date=short", "--decorate=short"];
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
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<usize> = Vec::new();
    let mut next_color = 0;

    for commit in commits {
        let before = lanes.clone();
        let incoming: Vec<usize> = before
            .iter()
            .enumerate()
            .filter_map(|(index, oid)| (oid.as_deref() == Some(&commit.oid)).then_some(index))
            .collect();

        let node = incoming.first().copied().unwrap_or_else(|| {
            if let Some(index) = lanes.iter().position(Option::is_none) {
                lanes[index] = Some(commit.oid.clone());
                colors[index] = next_color;
                next_color += 1;
                index
            } else {
                lanes.push(Some(commit.oid.clone()));
                colors.push(next_color);
                next_color += 1;
                lanes.len() - 1
            }
        });

        let mut after = lanes.clone();
        for lane in incoming.iter().copied().skip(1) {
            after[lane] = None;
        }

        if let Some(first_parent) = commit.parents.first() {
            after[node] = Some(first_parent.clone());
        } else {
            after[node] = None;
        }

        let mut outgoing = Vec::new();
        for parent in commit.parents.iter().skip(1) {
            let destination = after
                .iter()
                .position(|oid| oid.as_deref() == Some(parent))
                .unwrap_or_else(|| {
                    if let Some(index) = after.iter().position(Option::is_none) {
                        after[index] = Some(parent.clone());
                        colors[index] = next_color;
                        next_color += 1;
                        index
                    } else {
                        after.push(Some(parent.clone()));
                        colors.push(next_color);
                        next_color += 1;
                        after.len() - 1
                    }
                });
            outgoing.push(destination);
        }

        let lane_count = before.len().max(after.len()).max(node + 1);
        let mut masks = vec![0_u8; lane_count.saturating_mul(2).saturating_sub(1)];
        let mut cell_colors = vec![colors.get(node).copied().unwrap_or(0); masks.len()];

        for (index, lane) in before.iter().enumerate() {
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
        assert_eq!(repo.commits.len(), 4);
        assert_eq!(repo.history.len(), 4);
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
