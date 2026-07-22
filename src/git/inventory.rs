use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::Path,
    process::Stdio,
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail};

use super::{base_command, clean_stderr, run};

pub(super) fn git_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
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
    let (ignored_files, ignored_directories) = ignored_entries(root)?;
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
    let roots = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| path.strip_suffix(b"/"))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .filter(|path| !path.is_empty())
        .collect();
    let mut directories = expand_directories(root, roots)?;
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

fn ignored_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
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

fn expand_directories(root: &Path, roots: Vec<String>) -> Result<Vec<String>> {
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
        let ignored = ignored_paths(root, &candidates)?;
        frontier = candidates
            .into_iter()
            .filter(|path| !ignored.contains(path))
            .filter(|path| directories.insert(path.clone()))
            .collect();
    }
    Ok(directories.into_iter().collect())
}

fn ignored_paths(root: &Path, paths: &[String]) -> Result<HashSet<String>> {
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

pub(super) fn local_entries(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let started = Instant::now();
    crate::diagnostics::event(format!("local inventory started root={}", root.display()));
    let mut files = Vec::new();
    let mut directory_paths = Vec::new();
    let mut directories = vec![root.to_owned()];
    let mut next_progress = 100_000;
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
            let entries = files.len().saturating_add(directory_paths.len());
            if entries >= next_progress {
                crate::diagnostics::event(format!(
                    "local inventory progress root={} entries={} pending_directories={} elapsed_ms={}",
                    root.display(),
                    entries,
                    directories.len(),
                    started.elapsed().as_millis()
                ));
                next_progress = next_progress.saturating_add(100_000);
            }
        }
    }
    files.sort();
    files.dedup();
    directory_paths.sort();
    directory_paths.dedup();
    crate::diagnostics::event(format!(
        "local inventory finished root={} files={} directories={} elapsed_ms={}",
        root.display(),
        files.len(),
        directory_paths.len(),
        started.elapsed().as_millis()
    ));
    Ok((files, directory_paths))
}
