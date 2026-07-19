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
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    entries: Vec<usize>,
}

pub(crate) fn build_worktree(changes: &[Change], collapsed: &HashSet<String>) -> Vec<WorktreeRow> {
    let mut rows = Vec::new();
    append_worktree_section(changes, collapsed, WorktreeSection::Staged, &mut rows);
    append_worktree_section(changes, collapsed, WorktreeSection::Unstaged, &mut rows);
    rows
}

fn append_worktree_section(
    changes: &[Change],
    collapsed: &HashSet<String>,
    section: WorktreeSection,
    rows: &mut Vec<WorktreeRow>,
) {
    let mut root = Node::default();
    let mut count = 0;
    for (index, change) in changes.iter().enumerate() {
        let belongs = match section {
            WorktreeSection::Staged => change.staged,
            WorktreeSection::Unstaged => !change.staged,
        };
        if belongs {
            insert_path(&mut root, &change.path, index);
            count += 1;
        }
    }
    if count == 0 {
        return;
    }
    rows.push(WorktreeRow {
        prefix: String::new(),
        label: match section {
            WorktreeSection::Staged => format!("STAGED  {count}"),
            WorktreeSection::Unstaged => format!("UNSTAGED  {count}"),
        },
        depth: 0,
        change_index: None,
        directory_path: None,
        directory_expanded: None,
        section: Some(section),
    });
    flatten_worktree(&root, "", &[], true, collapsed, rows);
}

pub(crate) fn build_file_tree(files: &[String], collapsed: &HashSet<String>) -> Vec<ExplorerRow> {
    let mut root = Node::default();
    for (index, path) in files.iter().enumerate() {
        insert_path(&mut root, path, index);
    }
    let mut rows = Vec::new();
    flatten_file_tree(&root, "", &[], true, collapsed, &mut rows);
    rows
}

fn insert_path(root: &mut Node, path: &str, entry_index: usize) {
    let mut node = root;
    for component in path.split('/').filter(|component| !component.is_empty()) {
        node = node.children.entry(component.to_owned()).or_default();
    }
    node.entries.push(entry_index);
}

fn flatten_file_tree(
    node: &Node,
    parent_path: &str,
    lineage: &[bool],
    top_level: bool,
    collapsed: &HashSet<String>,
    rows: &mut Vec<ExplorerRow>,
) {
    let mut children: Vec<_> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| (child.children.is_empty(), name.as_str()));
    let child_count = children.len();
    for (position, (name, child)) in children.into_iter().enumerate() {
        let is_last = position + 1 == child_count;
        let first_root = top_level && position == 0;
        let mut path = join_path(parent_path, name);
        let prefix = tree_prefix(lineage, is_last, first_root);
        if child.children.is_empty() {
            if let Some(file_index) = child.entries.first() {
                rows.push(ExplorerRow {
                    prefix,
                    label: name.clone(),
                    depth: lineage.len(),
                    file_index: Some(*file_index),
                    directory_path: None,
                    directory_expanded: None,
                });
            }
            continue;
        }

        let mut label = name.clone();
        let mut directory = child;
        while directory.entries.is_empty() && directory.children.len() == 1 {
            let (next_name, next) = directory.children.first_key_value().expect("one child");
            if next.children.is_empty() {
                break;
            }
            label.push('/');
            label.push_str(next_name);
            path = join_path(&path, next_name);
            directory = next;
        }
        let expanded = !collapsed.contains(&path);
        rows.push(ExplorerRow {
            prefix,
            label,
            depth: lineage.len(),
            file_index: None,
            directory_path: Some(path.clone()),
            directory_expanded: Some(expanded),
        });
        if expanded {
            let mut child_lineage = lineage.to_vec();
            child_lineage.push(is_last);
            flatten_file_tree(directory, &path, &child_lineage, false, collapsed, rows);
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
    let mut children: Vec<_> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| (child.children.is_empty(), name.as_str()));
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
                });
            }
        } else {
            let mut label = name.clone();
            let mut directory = child;
            while directory.entries.is_empty() && directory.children.len() == 1 {
                let (next_name, next) = directory.children.first_key_value().expect("one child");
                if next.children.is_empty() {
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
            });
            if expanded {
                let mut child_lineage = lineage.to_vec();
                child_lineage.push(is_last);
                flatten_worktree(directory, &path, &child_lineage, false, collapsed, rows);
            }
        }
    }
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
                "UNSTAGED  3",
                "cli/crates/sleev-tui",
                "src",
                "views",
                "home.rs",
                "app.rs",
                "tests",
                "app.rs"
            ]
        );
        assert_eq!(rows[0].section, Some(WorktreeSection::Unstaged));
        assert_eq!(rows[1].prefix, " ");
        assert_eq!(rows[2].prefix, " │ ");
        assert_eq!(rows[3].label, "views");
        assert_eq!(rows[5].change_index, Some(0));
        assert_eq!(rows[4].change_index, Some(1));
        assert_eq!(rows[7].change_index, Some(2));

        let collapsed = HashSet::from(["cli/crates/sleev-tui".to_owned()]);
        let rows = build_worktree(&changes, &collapsed);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].directory_expanded, Some(false));
    }

    #[test]
    fn places_staged_changes_before_unstaged_changes() {
        let mut staged = change("src/app.rs");
        staged.staged = true;
        let rows = build_worktree(&[change("src/app.rs"), staged], &HashSet::new());

        assert_eq!(rows[0].section, Some(WorktreeSection::Staged));
        assert_eq!(rows[2].change_index, Some(1));
        assert_eq!(rows[3].section, Some(WorktreeSection::Unstaged));
        assert_eq!(rows[5].change_index, Some(0));
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
