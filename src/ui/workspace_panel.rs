use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    AgentStatus, HitTarget, WorkspaceDropTarget, WorkspacePanel, WorkspacePanelHitTarget,
    WorkspacePanelPlacement, WorkspacePanelRow,
};

use super::{fill, palette, truncate_width};

pub(super) fn draw(
    frame: &mut Frame<'_>,
    panel: &mut WorkspacePanel,
    area: Rect,
    focused: bool,
    hovered: Option<WorkspacePanelHitTarget>,
) -> Vec<(HitTarget, Rect)> {
    fill(frame, area, palette().surface_alt);
    let mut targets = Vec::new();
    if area.width < 4 || area.height == 0 {
        return targets;
    }

    let body = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let footer = Rect::new(
        area.x.saturating_add(1),
        area.bottom().saturating_sub(1),
        area.width.saturating_sub(2),
        1,
    );

    keep_selection_visible(panel, usize::from(body.height));
    let rows = panel.rows();
    let mut create_button = None;
    let mut snapshot_button = None;
    for (visual_row, row) in rows.iter().copied().enumerate().skip(panel.scroll) {
        let screen_row = visual_row.saturating_sub(panel.scroll);
        if screen_row >= usize::from(body.height) {
            break;
        }
        let row_area = Rect::new(body.x, body.y + screen_row as u16, body.width, 1);
        match row {
            WorkspacePanelRow::Header => {
                let (create, load) = draw_workspace_header(
                    frame,
                    row_area,
                    panel.create_menu_open || hovered == Some(WorkspacePanelHitTarget::CreateMenu),
                    panel.snapshot_menu_open
                        || hovered == Some(WorkspacePanelHitTarget::SnapshotMenu),
                );
                create_button = Some(create);
                targets.push((
                    HitTarget::WorkspacePanel(WorkspacePanelHitTarget::CreateMenu),
                    create,
                ));
                if let Some(load) = load {
                    snapshot_button = Some(load);
                    targets.push((
                        HitTarget::WorkspacePanel(WorkspacePanelHitTarget::SnapshotMenu),
                        load,
                    ));
                }
                let collapse = Rect::new(row_area.right().saturating_sub(1), row_area.y, 1, 1);
                let collapse_marker = match panel.placement {
                    WorkspacePanelPlacement::Right => "›",
                    WorkspacePanelPlacement::Off | WorkspacePanelPlacement::Left => "‹",
                };
                frame.render_widget(
                    Paragraph::new(collapse_marker).style(Style::default().fg(palette().faint)),
                    collapse,
                );
                targets.push((
                    HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Collapse),
                    collapse,
                ));
            }
            WorkspacePanelRow::Group(index) => {
                let group = &panel.groups[index];
                let count = panel
                    .workspaces
                    .iter()
                    .enumerate()
                    .filter(|(workspace, _)| panel.group_for_workspace(*workspace) == Some(index))
                    .count();
                let marker = if group.expanded { "▾" } else { "▸" };
                let drop_target =
                    panel.workspace_drag_target() == Some(WorkspaceDropTarget::Group(index));
                draw_group(frame, row_area, marker, &group.name, count, drop_target);
                targets.push((
                    HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Group(index)),
                    row_area,
                ));
            }
            WorkspacePanelRow::Workspace(index) => {
                let state = panel.workspace_entry_state(index, focused);
                let workspace = &panel.workspaces[index];
                let indent = panel.workspace_indent(index);
                let label = format!("{indent}{}", workspace.label);
                let ungrouped_drop = panel.workspace_drag_target()
                    == Some(WorkspaceDropTarget::Ungrouped)
                    && panel.group_for_workspace(index).is_none();
                draw_entry(
                    frame,
                    row_area,
                    &label,
                    workspace.branch.as_deref(),
                    workspace.status,
                    state.active,
                    state.selected || ungrouped_drop,
                );
                targets.push((
                    HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Workspace(index)),
                    row_area,
                ));
            }
            WorkspacePanelRow::Spacer => {}
            WorkspacePanelRow::AgentHeader => {
                draw_header(frame, row_area, "AGENTS", panel.agents.len());
            }
            WorkspacePanelRow::EmptyAgents => {
                frame.render_widget(
                    Paragraph::new("No agents detected")
                        .style(Style::default().fg(palette().faint)),
                    row_area,
                );
            }
            WorkspacePanelRow::Agent(index) => {
                let state = panel.agent_entry_state(index, focused);
                let agent = &panel.agents[index];
                let workspace = panel
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == agent.workspace_id)
                    .map_or("", |workspace| workspace.label.as_str());
                let label = if workspace.is_empty() {
                    agent.name.clone()
                } else {
                    format!("{} / {workspace}", agent.name)
                };
                draw_entry(
                    frame,
                    row_area,
                    &label,
                    None,
                    agent.status,
                    state.active,
                    state.selected,
                );
                targets.push((
                    HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Agent(index)),
                    row_area,
                ));
            }
        }
    }

    if panel.workspaces.is_empty() {
        let state = panel.error.as_deref().unwrap_or(if panel.loading {
            "Loading Herdr…"
        } else {
            "No workspaces"
        });
        if body.height > 1 {
            frame.render_widget(
                Paragraph::new(truncate_width(state, usize::from(body.width))).style(
                    Style::default().fg(if panel.error.is_some() {
                        palette().red
                    } else {
                        palette().faint
                    }),
                ),
                Rect::new(body.x, body.y.saturating_add(1), body.width, 1),
            );
        }
    }

    frame.render_widget(
        Paragraph::new(if panel.snapshot_editing {
            if let Some(error) = panel.snapshot_error.as_deref() {
                error
            } else {
                panel.snapshot_input.text()
            }
        } else if panel.group_editing {
            if let Some(error) = panel.group_error.as_deref() {
                error
            } else {
                panel.group_input.text()
            }
        } else if panel.snapshot_menu_open {
            panel
                .snapshot_error
                .as_deref()
                .unwrap_or("Enter load  Del remove")
        } else if focused {
            "Enter open  g group  Del"
        } else {
            "w move/off"
        })
        .style(Style::default().fg(if focused {
            palette().accent
        } else {
            palette().faint
        })),
        footer,
    );
    if panel.group_editing && panel.group_error.is_none() {
        frame.render_widget(
            Paragraph::new(format!("Group: {}", panel.group_input.text()))
                .style(Style::default().fg(palette().accent)),
            footer,
        );
    }
    if panel.snapshot_editing && panel.snapshot_error.is_none() {
        frame.render_widget(
            Paragraph::new(format!("Preset: {}", panel.snapshot_input.text()))
                .style(Style::default().fg(palette().accent)),
            footer,
        );
    }
    if panel.create_menu_open
        && let Some(anchor) = create_button
    {
        let worktree_enabled = panel.selected_workspace_id().is_some();
        let (workspace, worktree) = draw_create_popover(
            frame,
            body,
            anchor,
            panel.create_menu_choice,
            worktree_enabled,
            hovered,
        );
        targets.push((
            HitTarget::WorkspacePanel(WorkspacePanelHitTarget::CreateWorkspace),
            workspace,
        ));
        targets.push((
            HitTarget::WorkspacePanel(WorkspacePanelHitTarget::CreateWorktree),
            worktree,
        ));
    }
    if panel.snapshot_menu_open
        && let Some(anchor) = snapshot_button
    {
        for (index, area) in draw_snapshot_popover(frame, panel, body, anchor, hovered) {
            let target = if index == 0 {
                WorkspacePanelHitTarget::SaveSnapshot
            } else {
                WorkspacePanelHitTarget::Snapshot(index - 1)
            };
            targets.push((HitTarget::WorkspacePanel(target), area));
        }
    }
    targets
}

fn draw_snapshot_popover(
    frame: &mut Frame<'_>,
    panel: &WorkspacePanel,
    bounds: Rect,
    anchor: Rect,
    hovered: Option<WorkspacePanelHitTarget>,
) -> Vec<(usize, Rect)> {
    let item_count = panel.snapshots.len() + 1;
    let height = u16::try_from(item_count)
        .unwrap_or(u16::MAX)
        .min(bounds.height.saturating_sub(1));
    if height == 0 {
        return Vec::new();
    }
    let width = 23.min(bounds.width);
    let x = anchor
        .right()
        .saturating_sub(width)
        .clamp(bounds.x, bounds.right().saturating_sub(width));
    let y = anchor.bottom();
    let overlay = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, overlay);
    fill(frame, overlay, palette().raised);

    let visible = usize::from(height);
    let start = panel
        .snapshot_menu_choice
        .saturating_add(1)
        .saturating_sub(visible)
        .min(item_count.saturating_sub(visible));
    let mut areas = Vec::with_capacity(visible);
    for index in start..start + visible {
        let area = Rect::new(x, y + u16::try_from(index - start).unwrap_or(0), width, 1);
        let target = if index == 0 {
            WorkspacePanelHitTarget::SaveSnapshot
        } else {
            WorkspacePanelHitTarget::Snapshot(index - 1)
        };
        let hovered = hovered == Some(target);
        let selected = panel.snapshot_menu_choice == index;
        let label = if index == 0 {
            "Save current...".to_owned()
        } else {
            let snapshot = &panel.snapshots[index - 1];
            format!("{}  {}", snapshot.name, snapshot.workspace_count())
        };
        frame.render_widget(
            Paragraph::new(format!(
                "  {}",
                truncate_width(&label, usize::from(width).saturating_sub(2))
            ))
            .style(
                Style::default()
                    .fg(palette().ink)
                    .bg(if selected || hovered {
                        palette().selected
                    } else {
                        palette().raised
                    }),
            ),
            area,
        );
        areas.push((index, area));
    }
    areas
}

fn draw_create_popover(
    frame: &mut Frame<'_>,
    bounds: Rect,
    anchor: Rect,
    selection: usize,
    worktree_enabled: bool,
    hovered: Option<WorkspacePanelHitTarget>,
) -> (Rect, Rect) {
    let width = 18.min(bounds.width);
    let x = anchor
        .right()
        .saturating_sub(width)
        .clamp(bounds.x, bounds.right().saturating_sub(width));
    let y = anchor.bottom();
    let workspace = Rect::new(x, y, width, 1);
    let worktree = Rect::new(x, y.saturating_add(1), width, 1);
    let overlay = Rect::new(x, y, width, 2);
    frame.render_widget(ratatui::widgets::Clear, overlay);
    fill(frame, overlay, palette().raised);
    for (index, (label, area, enabled, target)) in [
        (
            "New workspace",
            workspace,
            true,
            WorkspacePanelHitTarget::CreateWorkspace,
        ),
        (
            "New worktree",
            worktree,
            worktree_enabled,
            WorkspacePanelHitTarget::CreateWorktree,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let active = enabled && (selection == index || hovered == Some(target));
        frame.render_widget(
            Paragraph::new(format!("  {label}")).style(
                Style::default()
                    .fg(if enabled {
                        palette().ink
                    } else {
                        palette().faint
                    })
                    .bg(if active {
                        palette().selected
                    } else {
                        palette().raised
                    }),
            ),
            area,
        );
    }
    (workspace, worktree)
}

fn draw_group(
    frame: &mut Frame<'_>,
    area: Rect,
    marker: &str,
    name: &str,
    count: usize,
    drop_target: bool,
) {
    let style = if drop_target {
        Style::default().bg(palette().selected).fg(palette().ink)
    } else {
        Style::default().fg(palette().muted)
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{marker} {}  {count}",
            truncate_width(name, usize::from(area.width).saturating_sub(6))
        ))
        .style(style.add_modifier(Modifier::BOLD)),
        area,
    );
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, label: &str, count: usize) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                label,
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {count}"), Style::default().fg(palette().faint)),
        ])),
        area,
    );
}

fn draw_workspace_header(
    frame: &mut Frame<'_>,
    area: Rect,
    create_hovered: bool,
    snapshot_hovered: bool,
) -> (Rect, Option<Rect>) {
    let compact = area.width < 22;
    let title = if compact { "WS" } else { "WORKSPACES" };
    let title_width = if compact { 2 } else { 10 };
    frame.render_widget(
        Paragraph::new(title).style(
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y, title_width.min(area.width), 1),
    );
    let button_x = area.x.saturating_add(title_width + 1);
    let button = Rect::new(
        button_x,
        area.y,
        3.min(area.right().saturating_sub(button_x)),
        1,
    );
    frame.render_widget(
        Paragraph::new(" + ").style(
            Style::default()
                .fg(if create_hovered {
                    palette().canvas
                } else {
                    palette().accent
                })
                .bg(if create_hovered {
                    palette().accent
                } else {
                    palette().raised
                })
                .add_modifier(Modifier::BOLD),
        ),
        button,
    );
    let load_x = button.right().saturating_add(1);
    let available = area.right().saturating_sub(1).saturating_sub(load_x);
    let load = (available >= 6).then(|| Rect::new(load_x, area.y, 6, 1));
    if let Some(load) = load {
        frame.render_widget(
            Paragraph::new(" Load ").style(
                Style::default()
                    .fg(if snapshot_hovered {
                        palette().canvas
                    } else {
                        palette().accent
                    })
                    .bg(if snapshot_hovered {
                        palette().accent
                    } else {
                        palette().raised
                    }),
            ),
            load,
        );
    }
    (button, load)
}

fn draw_entry(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    branch: Option<&str>,
    status: AgentStatus,
    active: bool,
    selected: bool,
) {
    let marker = if active { "› " } else { "  " };
    let status_marker = match status {
        AgentStatus::Unknown => "○",
        _ => "●",
    };
    let available = usize::from(area.width).saturating_sub(4);
    let branch = branch
        .filter(|branch| !branch.is_empty())
        .map(|branch| truncate_width(branch, available.saturating_sub(5).min(available / 2)));
    let branch_width = branch
        .as_deref()
        .map(UnicodeWidthStr::width)
        .unwrap_or_default();
    let label = truncate_width(
        label,
        available.saturating_sub(branch_width + usize::from(branch.is_some()) * 2),
    );
    let label_width = UnicodeWidthStr::width(label.as_str());
    let padding = available.saturating_sub(label_width + branch_width);
    let background = selected.then_some(palette().selected);
    let base = background.map_or_else(Style::default, |color| Style::default().bg(color));
    frame.render_widget(Block::default().style(base), area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                marker,
                base.fg(if active {
                    palette().accent
                } else {
                    palette().faint
                }),
            ),
            Span::styled(
                label,
                if active {
                    base.fg(palette().ink).add_modifier(Modifier::BOLD)
                } else {
                    base.fg(palette().muted)
                },
            ),
            Span::styled(" ".repeat(padding), base),
            Span::styled(branch.unwrap_or_default(), base.fg(palette().accent)),
            Span::styled(" ", base),
            Span::styled(status_marker, base.fg(status_color(status))),
        ])),
        area,
    );
}

fn status_color(status: AgentStatus) -> ratatui::style::Color {
    match status {
        AgentStatus::Idle => palette().cyan,
        AgentStatus::Working => palette().yellow,
        AgentStatus::Blocked => palette().red,
        AgentStatus::Done => palette().green,
        AgentStatus::Unknown => palette().faint,
    }
}

fn keep_selection_visible(panel: &mut WorkspacePanel, viewport: usize) {
    if viewport == 0 {
        return;
    }
    let Some(selected) = panel.selected_visual_row() else {
        panel.scroll = panel
            .scroll
            .min(panel.visual_row_count().saturating_sub(viewport));
        return;
    };
    if selected < panel.scroll {
        panel.scroll = selected;
    } else if selected >= panel.scroll.saturating_add(viewport) {
        panel.scroll = selected.saturating_add(1).saturating_sub(viewport);
    }
    panel.scroll = panel
        .scroll
        .min(panel.visual_row_count().saturating_sub(viewport));
}
