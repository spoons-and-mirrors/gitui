use std::{path::PathBuf, process::Command};

use serde_json::Value;

use super::{AgentStatus, HerdrAgent, HerdrWorkspace};

pub(super) struct Environment {
    pub(super) workspace_id: Option<String>,
}

pub(super) enum Action {
    CreateWorkspace { path: Option<PathBuf> },
    CreateWorktree { workspace_id: String },
    CloseWorkspace { workspace_id: String },
    RemoveWorktree { workspace_id: String },
    FocusWorkspace { workspace_id: String },
    FocusTab { tab_id: String },
    RenameWorkspace { workspace_id: String, label: String },
}

pub(super) struct RestoreRequest {
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) linked_worktree: bool,
}

struct ParsedWorkspace {
    workspace: HerdrWorkspace,
    repo_key: Option<String>,
}

#[cfg(not(test))]
pub(super) fn environment() -> Option<Environment> {
    environment_from(
        std::env::var("HERDR_ENV").ok().as_deref(),
        std::env::var("HERDR_WORKSPACE_ID").ok(),
    )
}

pub(super) fn perform(action: Action) -> Result<(), String> {
    run(&action_args(action)).map(|_| ())
}

pub(super) fn session_snapshot() -> Result<(Vec<HerdrWorkspace>, Vec<HerdrAgent>), String> {
    run(&["api".to_owned(), "snapshot".to_owned()]).and_then(|value| parse_snapshot(&value))
}

pub(super) fn restore(request: RestoreRequest) -> Result<Option<String>, String> {
    run(&restore_args(request)).map(|value| workspace_id_in(&value))
}

fn environment_from(enabled: Option<&str>, workspace_id: Option<String>) -> Option<Environment> {
    (enabled == Some("1")).then_some(Environment { workspace_id })
}

fn restore_args(request: RestoreRequest) -> Vec<String> {
    let mut args = if request.linked_worktree {
        vec![
            "worktree".to_owned(),
            "open".to_owned(),
            "--path".to_owned(),
        ]
    } else {
        vec![
            "workspace".to_owned(),
            "create".to_owned(),
            "--cwd".to_owned(),
        ]
    };
    args.push(request.path.to_string_lossy().into_owned());
    args.extend(["--label".to_owned(), request.label, "--no-focus".to_owned()]);
    args
}

fn action_args(action: Action) -> Vec<String> {
    match action {
        Action::CreateWorkspace { path } => {
            let mut args = vec!["workspace".to_owned(), "create".to_owned()];
            if let Some(path) = path {
                args.push("--cwd".to_owned());
                args.push(path.to_string_lossy().into_owned());
            }
            args.push("--no-focus".to_owned());
            args
        }
        Action::CreateWorktree { workspace_id } => vec![
            "worktree".to_owned(),
            "create".to_owned(),
            "--workspace".to_owned(),
            workspace_id,
            "--no-focus".to_owned(),
        ],
        Action::CloseWorkspace { workspace_id } => {
            vec!["workspace".to_owned(), "close".to_owned(), workspace_id]
        }
        Action::RemoveWorktree { workspace_id } => vec![
            "worktree".to_owned(),
            "remove".to_owned(),
            "--workspace".to_owned(),
            workspace_id,
        ],
        Action::FocusWorkspace { workspace_id } => {
            vec!["workspace".to_owned(), "focus".to_owned(), workspace_id]
        }
        Action::FocusTab { tab_id } => vec!["tab".to_owned(), "focus".to_owned(), tab_id],
        Action::RenameWorkspace {
            workspace_id,
            label,
        } => vec![
            "workspace".to_owned(),
            "rename".to_owned(),
            workspace_id,
            label,
        ],
    }
}

fn run(args: &[String]) -> Result<Value, String> {
    let output = Command::new("herdr")
        .args(args)
        .output()
        .map_err(|error| format!("Herdr unavailable: {error}"))?;
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().find(|line| !line.trim().is_empty());
        detail.map_or_else(
            || format!("Could not read Herdr response: {error}"),
            |detail| detail.trim().to_owned(),
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Herdr command failed")
            .to_owned());
    }
    if !output.status.success() {
        return Err("Herdr command failed".to_owned());
    }
    Ok(value)
}

pub(super) fn parse_snapshot(
    value: &Value,
) -> Result<(Vec<HerdrWorkspace>, Vec<HerdrAgent>), String> {
    let snapshot = value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .ok_or_else(|| "Herdr returned an invalid session snapshot".to_owned())?;
    let mut workspaces: Vec<ParsedWorkspace> = snapshot
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "Herdr snapshot has no workspaces".to_owned())?
        .iter()
        .filter_map(|workspace| parse_workspace(workspace, snapshot))
        .collect();
    assign_worktree_parents(&mut workspaces);
    let agents = snapshot
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_agent)
        .collect();
    Ok((
        workspaces
            .into_iter()
            .map(|parsed| parsed.workspace)
            .collect(),
        agents,
    ))
}

fn parse_workspace(value: &Value, snapshot: &Value) -> Option<ParsedWorkspace> {
    let worktree = value.get("worktree").filter(|value| value.is_object());
    Some(ParsedWorkspace {
        repo_key: worktree
            .and_then(|worktree| worktree.get("repo_key"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        workspace: HerdrWorkspace {
            id: value.get("workspace_id")?.as_str()?.to_owned(),
            label: value.get("label")?.as_str()?.to_owned(),
            path: workspace_path(value, snapshot),
            branch: None,
            parent_workspace_id: None,
            pane_count: value.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as usize,
            focused: value
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            status: parse_agent_status(value.get("agent_status").and_then(Value::as_str)),
            repo_root: worktree
                .and_then(|worktree| worktree.get("repo_root"))
                .and_then(Value::as_str)
                .map(PathBuf::from),
            linked_worktree: worktree
                .and_then(|worktree| worktree.get("is_linked_worktree"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    })
}

fn assign_worktree_parents(workspaces: &mut [ParsedWorkspace]) {
    let parent_ids = workspaces
        .iter()
        .map(|worktree| {
            if !worktree.workspace.linked_worktree {
                return None;
            }
            let repo_key = worktree.repo_key.as_deref()?;
            let exact_root = workspaces.iter().find(|candidate| {
                !candidate.workspace.linked_worktree
                    && candidate.workspace.path.as_deref()
                        == worktree.workspace.repo_root.as_deref()
            });
            exact_root
                .or_else(|| {
                    workspaces.iter().find(|candidate| {
                        !candidate.workspace.linked_worktree
                            && candidate.repo_key.as_deref() == Some(repo_key)
                    })
                })
                .map(|parent| parent.workspace.id.clone())
        })
        .collect::<Vec<_>>();
    for (workspace, parent_id) in workspaces.iter_mut().zip(parent_ids) {
        workspace.workspace.parent_workspace_id = parent_id;
    }
}

fn workspace_path(workspace: &Value, snapshot: &Value) -> Option<PathBuf> {
    if let Some(path) = workspace
        .get("worktree")
        .and_then(|worktree| worktree.get("checkout_path"))
        .and_then(Value::as_str)
    {
        return Some(PathBuf::from(path));
    }

    let workspace_id = workspace.get("workspace_id")?.as_str()?;
    let active_tab_id = workspace.get("active_tab_id").and_then(Value::as_str);
    let panes = snapshot.get("panes").and_then(Value::as_array)?;
    let focused_pane_id = snapshot
        .get("layouts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|layout| {
            layout.get("workspace_id").and_then(Value::as_str) == Some(workspace_id)
                && layout.get("tab_id").and_then(Value::as_str) == active_tab_id
        })
        .and_then(|layout| layout.get("focused_pane_id"))
        .and_then(Value::as_str);
    let workspace_panes = || {
        panes
            .iter()
            .filter(|pane| pane.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
    };
    let pane = focused_pane_id
        .and_then(|focused| {
            workspace_panes()
                .find(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(focused))
        })
        .or_else(|| {
            active_tab_id.and_then(|active_tab| {
                workspace_panes()
                    .find(|pane| pane.get("tab_id").and_then(Value::as_str) == Some(active_tab))
            })
        })
        .or_else(|| workspace_panes().next())?;
    pane.get("foreground_cwd")
        .and_then(Value::as_str)
        .or_else(|| pane.get("cwd").and_then(Value::as_str))
        .map(PathBuf::from)
}

fn parse_agent(value: &Value) -> Option<HerdrAgent> {
    Some(HerdrAgent {
        name: value.get("agent")?.as_str()?.to_owned(),
        workspace_id: value.get("workspace_id")?.as_str()?.to_owned(),
        tab_id: value.get("tab_id")?.as_str()?.to_owned(),
        pane_id: value.get("pane_id")?.as_str()?.to_owned(),
        focused: value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: parse_agent_status(value.get("agent_status").and_then(Value::as_str)),
    })
}

fn parse_agent_status(value: Option<&str>) -> AgentStatus {
    match value {
        Some("idle") => AgentStatus::Idle,
        Some("working") => AgentStatus::Working,
        Some("blocked") => AgentStatus::Blocked,
        Some("done") => AgentStatus::Done,
        _ => AgentStatus::Unknown,
    }
}

fn workspace_id_in(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get("workspace_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| object.values().find_map(workspace_id_in)),
        Value::Array(values) => values.iter().find_map(workspace_id_in),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn builds_typed_action_and_restore_arguments() {
        assert_eq!(
            action_args(Action::CreateWorkspace {
                path: Some(PathBuf::from("/tmp/current workspace")),
            }),
            [
                "workspace",
                "create",
                "--cwd",
                "/tmp/current workspace",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::CreateWorktree {
                workspace_id: "w1".to_owned(),
            }),
            ["worktree", "create", "--workspace", "w1", "--no-focus"].map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::CloseWorkspace {
                workspace_id: "w1".to_owned(),
            }),
            ["workspace", "close", "w1"].map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::RemoveWorktree {
                workspace_id: "w3".to_owned(),
            }),
            ["worktree", "remove", "--workspace", "w3"].map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::FocusWorkspace {
                workspace_id: "w1".to_owned(),
            }),
            ["workspace", "focus", "w1"].map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::FocusTab {
                tab_id: "w1:t2".to_owned(),
            }),
            ["tab", "focus", "w1:t2"].map(str::to_owned)
        );
        assert_eq!(
            action_args(Action::RenameWorkspace {
                workspace_id: "w1".to_owned(),
                label: "code".to_owned(),
            }),
            ["workspace", "rename", "w1", "code"].map(str::to_owned)
        );
        assert_eq!(
            restore_args(RestoreRequest {
                path: PathBuf::from("/tmp/code"),
                label: "Code".to_owned(),
                linked_worktree: false,
            }),
            [
                "workspace",
                "create",
                "--cwd",
                "/tmp/code",
                "--label",
                "Code",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            restore_args(RestoreRequest {
                path: PathBuf::from("/tmp/feature"),
                label: "Feature".to_owned(),
                linked_worktree: true,
            }),
            [
                "worktree",
                "open",
                "--path",
                "/tmp/feature",
                "--label",
                "Feature",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn detects_environment_and_nested_workspace_ids() {
        assert!(environment_from(Some("0"), Some("w1".to_owned())).is_none());
        assert_eq!(
            environment_from(Some("1"), Some("w1".to_owned()))
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("w1")
        );
        let response = serde_json::json!({
            "result": { "event": { "workspace": { "workspace_id": "workspace-42" } } }
        });
        assert_eq!(workspace_id_in(&response).as_deref(), Some("workspace-42"));
    }

    #[test]
    fn parses_paths_statuses_and_repo_key_parent_fallback() {
        let value = serde_json::json!({
            "result": { "snapshot": {
                "workspaces": [
                    {
                        "workspace_id": "parent",
                        "label": "Parent",
                        "pane_count": 1,
                        "agent_status": "working",
                        "worktree": {
                            "checkout_path": "/repos/project",
                            "repo_key": "project.git",
                            "repo_root": "/repos/project",
                            "is_linked_worktree": false
                        }
                    },
                    {
                        "workspace_id": "child",
                        "label": "Child",
                        "pane_count": 1,
                        "agent_status": "idle",
                        "worktree": {
                            "checkout_path": "/worktrees/feature",
                            "repo_key": "project.git",
                            "repo_root": "/different/root",
                            "is_linked_worktree": true
                        }
                    },
                    {
                        "workspace_id": "pane-path",
                        "label": "Pane path",
                        "active_tab_id": "tab-3",
                        "pane_count": 1,
                        "agent_status": "done"
                    }
                ],
                "agents": [{
                    "agent": "opencode",
                    "agent_status": "blocked",
                    "focused": true,
                    "pane_id": "pane-3",
                    "tab_id": "tab-3",
                    "workspace_id": "pane-path"
                }],
                "panes": [{
                    "pane_id": "pane-3",
                    "tab_id": "tab-3",
                    "workspace_id": "pane-path",
                    "cwd": "/fallback",
                    "foreground_cwd": "/foreground"
                }],
                "layouts": []
            }}
        });

        let (workspaces, agents) = parse_snapshot(&value).unwrap();
        assert_eq!(workspaces[0].status, AgentStatus::Working);
        assert_eq!(workspaces[1].parent_workspace_id.as_deref(), Some("parent"));
        assert_eq!(
            workspaces[2].path.as_deref(),
            Some(Path::new("/foreground"))
        );
        assert_eq!(workspaces[2].status, AgentStatus::Done);
        assert_eq!(agents[0].status, AgentStatus::Blocked);
    }
}
