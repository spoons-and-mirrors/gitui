use std::{
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub(crate) enum FileOperation {
    CreateFile { path: String },
    CreateDirectory { path: String },
    Rename { from: String, to: String },
    Move { from: String, to: String },
    Delete { path: String },
}

impl FileOperation {
    pub(crate) fn selection_after(&self) -> Option<String> {
        match self {
            Self::CreateFile { path } | Self::CreateDirectory { path } => Some(path.clone()),
            Self::Rename { to, .. } | Self::Move { to, .. } => Some(to.clone()),
            Self::Delete { .. } => None,
        }
    }

    pub(crate) fn success_message(&self) -> String {
        match self {
            Self::CreateFile { path } => format!("Created {path}"),
            Self::CreateDirectory { path } => format!("Created {path}/"),
            Self::Rename { to, .. } => format!("Renamed to {to}"),
            Self::Move { to, .. } => format!("Moved to {to}"),
            Self::Delete { path } => format!("Deleted {path}"),
        }
    }
}

pub(crate) fn perform(root: &Path, operation: &FileOperation) -> Result<()> {
    match operation {
        FileOperation::CreateFile { path } => {
            let path = safe_path(root, path)?;
            ensure_parent_directory(&path)?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| format!("could not create {}", path.display()))?;
        }
        FileOperation::CreateDirectory { path } => {
            let path = safe_path(root, path)?;
            ensure_parent_directory(&path)?;
            fs::create_dir(&path)
                .with_context(|| format!("could not create directory {}", path.display()))?;
        }
        FileOperation::Rename { from, to } | FileOperation::Move { from, to } => {
            let from_path = safe_path(root, from)?;
            let to_path = safe_path(root, to)?;
            if from_path == to_path {
                return Ok(());
            }
            let metadata = fs::symlink_metadata(&from_path)
                .with_context(|| format!("could not inspect {}", from_path.display()))?;
            if metadata.is_dir() && fs::symlink_metadata(from_path.join(".git")).is_ok() {
                bail!("moving a nested Git repository or submodule is not supported");
            }
            if fs::symlink_metadata(&to_path).is_ok() {
                bail!("{} already exists", to_path.display());
            }
            if metadata.is_dir() && to_path.starts_with(&from_path) {
                bail!("cannot move a directory into itself");
            }
            ensure_parent_directory(&to_path)?;
            fs::rename(&from_path, &to_path).with_context(|| {
                format!(
                    "could not move {} to {}",
                    from_path.display(),
                    to_path.display()
                )
            })?;
        }
        FileOperation::Delete { path } => {
            let path = safe_path(root, path)?;
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("could not inspect {}", path.display()))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("could not delete directory {}", path.display()))?;
            } else {
                fs::remove_file(&path)
                    .with_context(|| format!("could not delete file {}", path.display()))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Name cannot be empty");
    }
    if name == "." || name == ".." || name == ".git" {
        bail!("That name is not allowed");
    }
    if name.contains(['/', '\\']) {
        bail!("Enter a name, not a path");
    }
    if Path::new(name).components().count() != 1 {
        bail!("That name is not valid");
    }
    Ok(())
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let root_metadata = fs::symlink_metadata(root)
        .with_context(|| format!("could not inspect workspace {}", root.display()))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        bail!("the workspace root is no longer a safe directory");
    }
    let path = Path::new(relative);
    if relative.is_empty() || path.is_absolute() {
        bail!("Invalid workspace path");
    }
    let components: Vec<_> = path.components().collect();
    for component in &components {
        match component {
            Component::Normal(value) if *value != ".git" => {}
            _ => bail!("Path must stay inside the workspace"),
        }
    }
    let mut ancestor = root.to_owned();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(value) = component else {
            unreachable!("components were validated above")
        };
        ancestor.push(value);
        let metadata = fs::symlink_metadata(&ancestor)
            .with_context(|| format!("could not inspect {}", ancestor.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("{} is not a safe workspace directory", ancestor.display());
        }
    }
    Ok(root.join(path))
}

fn ensure_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("could not inspect {}", parent.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("{} is not a directory", parent.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_moves_renames_and_deletes_workspace_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();

        perform(
            root,
            &FileOperation::CreateDirectory {
                path: "docs".to_owned(),
            },
        )
        .unwrap();
        perform(
            root,
            &FileOperation::CreateFile {
                path: "readme.md".to_owned(),
            },
        )
        .unwrap();
        perform(
            root,
            &FileOperation::Move {
                from: "readme.md".to_owned(),
                to: "docs/readme.md".to_owned(),
            },
        )
        .unwrap();
        perform(
            root,
            &FileOperation::Rename {
                from: "docs/readme.md".to_owned(),
                to: "docs/guide.md".to_owned(),
            },
        )
        .unwrap();
        assert!(root.join("docs/guide.md").is_file());

        perform(
            root,
            &FileOperation::Delete {
                path: "docs".to_owned(),
            },
        )
        .unwrap();
        assert!(!root.join("docs").exists());
    }

    #[test]
    fn rejects_traversal_overwrites_and_descendant_moves() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("source")).unwrap();
        fs::write(root.join("source/file"), "content").unwrap();
        fs::write(root.join("existing"), "content").unwrap();
        fs::create_dir_all(root.join("nested-repository/.git")).unwrap();

        assert!(
            perform(
                root,
                &FileOperation::Delete {
                    path: "../outside".to_owned()
                }
            )
            .is_err()
        );
        assert!(
            perform(
                root,
                &FileOperation::Move {
                    from: "nested-repository".to_owned(),
                    to: "moved-repository".to_owned(),
                },
            )
            .is_err()
        );
        assert!(
            perform(
                root,
                &FileOperation::Delete {
                    path: ".git/config".to_owned()
                }
            )
            .is_err()
        );
        assert!(
            perform(
                root,
                &FileOperation::Rename {
                    from: "source/file".to_owned(),
                    to: "existing".to_owned(),
                },
            )
            .is_err()
        );
        assert!(
            perform(
                root,
                &FileOperation::Move {
                    from: "source".to_owned(),
                    to: "source/nested/source".to_owned(),
                },
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/keep"), "content").unwrap();
        symlink(root.join("target"), root.join("link")).unwrap();

        perform(
            root,
            &FileOperation::Delete {
                path: "link".to_owned(),
            },
        )
        .unwrap();
        assert!(root.join("target/keep").exists());
        assert!(!root.join("link").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_operations_through_a_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), "content").unwrap();
        symlink(outside.path(), workspace.path().join("link")).unwrap();

        assert!(
            perform(
                workspace.path(),
                &FileOperation::Delete {
                    path: "link/keep".to_owned(),
                },
            )
            .is_err()
        );
        assert!(
            perform(
                workspace.path(),
                &FileOperation::CreateFile {
                    path: "link/new".to_owned(),
                },
            )
            .is_err()
        );
        assert!(outside.path().join("keep").exists());
        assert!(!outside.path().join("new").exists());

        let container = tempfile::tempdir().unwrap();
        let root = container.path().join("workspace");
        let original = container.path().join("original");
        fs::create_dir(&root).unwrap();
        fs::rename(&root, &original).unwrap();
        symlink(outside.path(), &root).unwrap();
        assert!(
            perform(
                &root,
                &FileOperation::Delete {
                    path: "keep".to_owned(),
                },
            )
            .is_err()
        );
        assert!(outside.path().join("keep").exists());
    }
}
