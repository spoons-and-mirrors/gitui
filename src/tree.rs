use std::collections::{BTreeMap, HashSet};

use crate::git::Change;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeRow {
    pub(crate) prefix: String,
    pub(crate) label: String,
    pub(crate) depth: usize,
    pub(crate) change_index: Option<usize>,
    pub(crate) directory_path: Option<String>,
    pub(crate) directory_expanded: Option<bool>,
    pub(crate) section: Option<WorktreeSection>,
    pub(crate) section_stats: Option<(u64, u64)>,
    pub(crate) descendant_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeSection {
    Staged,
    Unstaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplorerRow {
    pub(crate) prefix: String,
    pub(crate) label: String,
    pub(crate) depth: usize,
    pub(crate) file_index: Option<usize>,
    pub(crate) directory_path: Option<String>,
    pub(crate) directory_expanded: Option<bool>,
    pub(crate) descendant_count: usize,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    entries: Vec<usize>,
    descendant_count: usize,
    explicit_directory: bool,
}

pub(crate) struct FileTree {
    root: Node,
}

pub(crate) struct PreparedFileTree {
    tree: FileTree,
}

impl PreparedFileTree {
    pub(crate) fn new(files: &[String], directories: &[String]) -> Self {
        Self {
            tree: FileTree::new(files, directories),
        }
    }

    pub(crate) fn into_tree(self) -> FileTree {
        self.tree
    }
}

impl FileTree {
    pub(crate) fn new(files: &[String], directories: &[String]) -> Self {
        let _activity = crate::diagnostics::activity(
            "build-file-tree",
            format!("files={} directories={}", files.len(), directories.len()),
        );
        let mut root = Node::default();
        for (index, path) in files.iter().enumerate() {
            insert_path(&mut root, path, index);
        }
        for path in directories {
            insert_directory(&mut root, path);
        }
        Self { root }
    }

    #[cfg(test)]
    fn rows(&self, collapsed: &HashSet<String>) -> Vec<ExplorerRow> {
        let mut rows = Vec::new();
        flatten_file_tree(
            &self.root,
            "",
            &[],
            true,
            &|path| !collapsed.contains(path),
            &mut rows,
        );
        rows
    }

    pub(crate) fn rows_expanded(&self, expanded: &HashSet<String>) -> Vec<ExplorerRow> {
        let mut rows = Vec::new();
        flatten_file_tree(
            &self.root,
            "",
            &[],
            true,
            &|path| expanded.contains(path),
            &mut rows,
        );
        rows
    }
}

struct WorktreeSectionTree {
    section: WorktreeSection,
    root: Node,
    additions: u64,
    deletions: u64,
}

pub(crate) struct WorktreeTree {
    sections: Vec<WorktreeSectionTree>,
}

impl WorktreeTree {
    pub(crate) fn new(changes: &[Change]) -> Self {
        let mut sections = Vec::new();
        for section in [WorktreeSection::Staged, WorktreeSection::Unstaged] {
            let mut root = Node::default();
            let mut additions = 0_u64;
            let mut deletions = 0_u64;
            for (index, change) in changes.iter().enumerate() {
                let belongs = match section {
                    WorktreeSection::Staged => change.staged,
                    WorktreeSection::Unstaged => !change.staged,
                };
                if belongs {
                    insert_path(&mut root, &change.path, index);
                    additions = additions.saturating_add(change.additions);
                    deletions = deletions.saturating_add(change.deletions);
                }
            }
            if root.descendant_count > 0 {
                sections.push(WorktreeSectionTree {
                    section,
                    root,
                    additions,
                    deletions,
                });
            }
        }
        Self { sections }
    }

    pub(crate) fn rows(&self, collapsed: &HashSet<String>) -> Vec<WorktreeRow> {
        let mut rows = Vec::new();
        for tree in &self.sections {
            append_worktree_section(tree, collapsed, &mut rows);
        }
        rows
    }
}

fn append_worktree_section(
    tree: &WorktreeSectionTree,
    collapsed: &HashSet<String>,
    rows: &mut Vec<WorktreeRow>,
) {
    rows.push(WorktreeRow {
        prefix: String::new(),
        label: String::new(),
        depth: 0,
        change_index: None,
        directory_path: None,
        directory_expanded: None,
        section: Some(tree.section),
        section_stats: None,
        descendant_count: tree.root.descendant_count,
    });
    rows.push(WorktreeRow {
        prefix: String::new(),
        label: match tree.section {
            WorktreeSection::Staged => "STAGED".to_owned(),
            WorktreeSection::Unstaged => "UNSTAGED".to_owned(),
        },
        depth: 0,
        change_index: None,
        directory_path: None,
        directory_expanded: None,
        section: Some(tree.section),
        section_stats: Some((tree.additions, tree.deletions)),
        descendant_count: tree.root.descendant_count,
    });
    flatten_worktree(&tree.root, "", &[], true, collapsed, rows);
}

fn insert_path(root: &mut Node, path: &str, entry_index: usize) {
    let mut node = root;
    node.descendant_count += 1;
    for component in path.split('/').filter(|component| !component.is_empty()) {
        node = node.children.entry(component.to_owned()).or_default();
        node.descendant_count += 1;
    }
    node.entries.push(entry_index);
}

fn insert_directory(root: &mut Node, path: &str) {
    let mut node = root;
    for component in path.split('/').filter(|component| !component.is_empty()) {
        node = node.children.entry(component.to_owned()).or_default();
    }
    node.explicit_directory = true;
}

fn sorted_children(node: &Node) -> Vec<(&String, &Node)> {
    let mut children: Vec<_> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| {
        (
            child.children.is_empty() && !child.explicit_directory,
            name.as_str(),
        )
    });
    children
}

fn flatten_file_tree(
    node: &Node,
    parent_path: &str,
    lineage: &[bool],
    top_level: bool,
    is_expanded: &impl Fn(&str) -> bool,
    rows: &mut Vec<ExplorerRow>,
) {
    let children = sorted_children(node);
    let child_count = children.len();
    for (position, (name, child)) in children.into_iter().enumerate() {
        let is_last = position + 1 == child_count;
        let first_root = top_level && position == 0;
        let mut path = join_path(parent_path, name);
        let prefix = tree_prefix(lineage, is_last, first_root);
        if child.children.is_empty() && !child.explicit_directory {
            if let Some(file_index) = child.entries.first() {
                rows.push(ExplorerRow {
                    prefix,
                    label: name.clone(),
                    depth: lineage.len(),
                    file_index: Some(*file_index),
                    directory_path: None,
                    directory_expanded: None,
                    descendant_count: 1,
                });
            }
            continue;
        }

        let mut label = name.clone();
        let mut directory = child;
        while !directory.explicit_directory
            && directory.entries.is_empty()
            && directory.children.len() == 1
        {
            let (next_name, next) = directory.children.first_key_value().expect("one child");
            if next.children.is_empty() {
                break;
            }
            label.push('/');
            label.push_str(next_name);
            path = join_path(&path, next_name);
            directory = next;
        }
        let expanded = is_expanded(&path);
        rows.push(ExplorerRow {
            prefix,
            label,
            depth: lineage.len(),
            file_index: None,
            directory_path: Some(path.clone()),
            directory_expanded: Some(expanded),
            descendant_count: directory.descendant_count,
        });
        if expanded {
            let mut child_lineage = lineage.to_vec();
            child_lineage.push(is_last);
            flatten_file_tree(directory, &path, &child_lineage, false, is_expanded, rows);
        }
    }
}

fn flatten_worktree(
    node: &Node,
    parent_path: &str,
    lineage: &[bool],
    top_level: bool,
    collapsed: &HashSet<String>,
    rows: &mut Vec<WorktreeRow>,
) {
    let children = sorted_children(node);
    let child_count = children.len();
    for (position, (name, child)) in children.into_iter().enumerate() {
        let is_last = position + 1 == child_count;
        let first_root = top_level && position == 0;
        let mut path = join_path(parent_path, name);
        let prefix = tree_prefix(lineage, is_last, first_root);

        if child.children.is_empty() {
            for (duplicate, change_index) in child.entries.iter().enumerate() {
                rows.push(WorktreeRow {
                    prefix: if duplicate == 0 {
                        prefix.clone()
                    } else {
                        tree_prefix(lineage, is_last, first_root)
                    },
                    label: name.clone(),
                    depth: lineage.len(),
                    change_index: Some(*change_index),
                    directory_path: None,
                    directory_expanded: None,
                    section: None,
                    section_stats: None,
                    descendant_count: 1,
                });
            }
        } else {
            for change_index in &child.entries {
                rows.push(WorktreeRow {
                    prefix: prefix.clone(),
                    label: name.clone(),
                    depth: lineage.len(),
                    change_index: Some(*change_index),
                    directory_path: None,
                    directory_expanded: None,
                    section: None,
                    section_stats: None,
                    descendant_count: 1,
                });
            }
            let mut label = name.clone();
            let mut directory = child;
            while directory.entries.is_empty() && directory.children.len() == 1 {
                let (next_name, next) = directory.children.first_key_value().expect("one child");
                if next.children.is_empty() || !next.entries.is_empty() {
                    break;
                }
                label.push('/');
                label.push_str(next_name);
                path = join_path(&path, next_name);
                directory = next;
            }
            let expanded = !collapsed.contains(&path);
            rows.push(WorktreeRow {
                prefix,
                label,
                depth: lineage.len(),
                change_index: None,
                directory_path: Some(path.clone()),
                directory_expanded: Some(expanded),
                section: None,
                section_stats: None,
                descendant_count: directory
                    .descendant_count
                    .saturating_sub(directory.entries.len()),
            });
            if expanded {
                let mut child_lineage = lineage.to_vec();
                child_lineage.push(is_last);
                flatten_worktree(directory, &path, &child_lineage, false, collapsed, rows);
            }
        }
    }
}

#[cfg(test)]
fn build_worktree(changes: &[Change], collapsed: &HashSet<String>) -> Vec<WorktreeRow> {
    WorktreeTree::new(changes).rows(collapsed)
}

#[cfg(test)]
fn build_file_tree(files: &[String], collapsed: &HashSet<String>) -> Vec<ExplorerRow> {
    FileTree::new(files, &[]).rows(collapsed)
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn tree_prefix(lineage: &[bool], _is_last: bool, _first_root: bool) -> String {
    let mut prefix = String::from(" ");
    for _ in lineage {
        prefix.push_str("│ ");
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_hierarchical_worktree_without_repeating_paths() {
        let changes = [
            change("cli/crates/sleev-tui/src/app.rs"),
            change("cli/crates/sleev-tui/src/views/home.rs"),
            change("cli/crates/sleev-tui/tests/app.rs"),
        ];

        let rows = build_worktree(&changes, &HashSet::new());
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "",
                "UNSTAGED",
                "cli/crates/sleev-tui",
                "src",
                "views",
                "home.rs",
                "app.rs",
                "tests",
                "app.rs"
            ]
        );
        assert_eq!(rows[1].section, Some(WorktreeSection::Unstaged));
        assert_eq!(rows[2].prefix, " ");
        assert_eq!(rows[3].prefix, " │ ");
        assert_eq!(rows[4].label, "views");
        assert_eq!(rows[6].change_index, Some(0));
        assert_eq!(rows[5].change_index, Some(1));
        assert_eq!(rows[8].change_index, Some(2));

        let collapsed = HashSet::from(["cli/crates/sleev-tui".to_owned()]);
        let rows = build_worktree(&changes, &collapsed);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].directory_expanded, Some(false));
    }

    #[test]
    fn places_staged_changes_before_unstaged_changes() {
        let mut staged = change("src/app.rs");
        staged.staged = true;
        staged.additions = 4;
        staged.deletions = 1;
        let mut unstaged = change("src/app.rs");
        unstaged.additions = 2;
        unstaged.deletions = 1;
        let rows = build_worktree(&[unstaged, staged], &HashSet::new());

        assert_eq!(rows[1].section, Some(WorktreeSection::Staged));
        assert_eq!(rows[1].label, "STAGED");
        assert_eq!(rows[1].section_stats, Some((4, 1)));
        assert_eq!(rows[3].change_index, Some(1));
        assert_eq!(rows[5].section, Some(WorktreeSection::Unstaged));
        assert_eq!(rows[5].label, "UNSTAGED");
        assert_eq!(rows[5].section_stats, Some((2, 1)));
        assert_eq!(rows[7].change_index, Some(0));
    }

    #[test]
    fn keeps_a_deleted_file_alongside_an_untracked_directory_at_the_same_path() {
        let changes = [
            change("foo"),
            change("foo/bar.txt"),
            change("dir/nested"),
            change("dir/nested/child.txt"),
        ];

        let rows = build_worktree(&changes, &HashSet::new());

        assert!(rows.iter().any(|row| row.change_index == Some(0)));
        assert!(rows.iter().any(|row| row.change_index == Some(1)));
        assert!(rows.iter().any(|row| row.change_index == Some(2)));
        assert!(rows.iter().any(|row| row.change_index == Some(3)));
        let directory = rows
            .iter()
            .find(|row| row.directory_path.as_deref() == Some("foo"))
            .unwrap();
        assert_eq!(directory.descendant_count, 1);
    }

    #[test]
    fn builds_a_collapsible_repository_file_tree() {
        let files = vec![
            "src/app/mod.rs".to_owned(),
            "src/app/view.rs".to_owned(),
            "src/main.rs".to_owned(),
            "README.md".to_owned(),
        ];
        let rows = build_file_tree(&files, &HashSet::new());
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            ["src", "app", "mod.rs", "view.rs", "main.rs", "README.md"]
        );
        assert_eq!(rows[2].file_index, Some(0));
        assert_eq!(rows[4].file_index, Some(2));

        let rows = build_file_tree(&files, &HashSet::from(["src/app".to_owned()]));
        assert!(rows.iter().any(|row| {
            row.directory_path.as_deref() == Some("src/app")
                && row.directory_expanded == Some(false)
        }));
        assert!(!rows.iter().any(|row| row.label == "mod.rs"));
    }

    #[test]
    fn keeps_explicit_empty_directories_in_the_file_tree() {
        let tree = FileTree::new(
            &["src/main.rs".to_owned()],
            &[
                "empty".to_owned(),
                "src/nested/empty".to_owned(),
                "a".to_owned(),
                "a/b".to_owned(),
                "a/b/c".to_owned(),
            ],
        );
        let rows = tree.rows(&HashSet::new());

        assert!(rows.iter().any(|row| {
            row.directory_path.as_deref() == Some("empty") && row.file_index.is_none()
        }));
        assert!(rows.iter().any(|row| {
            row.directory_path.as_deref() == Some("src/nested/empty") && row.file_index.is_none()
        }));
        for path in ["a", "a/b", "a/b/c"] {
            assert!(rows.iter().any(|row| {
                row.directory_path.as_deref() == Some(path) && row.file_index.is_none()
            }));
        }
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.to_owned(),
            original_path: None,
            code: 'M',
            staged: false,
            additions: 0,
            deletions: 0,
        }
    }
}
