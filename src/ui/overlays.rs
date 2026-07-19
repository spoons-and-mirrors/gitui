use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    ACTION_ITEMS, ActionsState, CommandStatus, PickerAction, PickerEntry, RepositoryPicker,
    Settings,
};

use super::{fill, palette, truncate_width};

pub(super) struct PickerRegions {
    pub(super) overlay: Rect,
    pub(super) path: Rect,
    pub(super) list: Rect,
}

pub(super) struct SettingsRegions {
    pub(super) overlay: Rect,
    pub(super) auto_fetch: Rect,
    pub(super) fetch_interval: Rect,
    pub(super) fetch_interval_down: Rect,
    pub(super) fetch_interval_up: Rect,
    pub(super) editor: Rect,
}

pub(super) struct ActionMenuRegions {
    pub(super) overlay: Rect,
    pub(super) list: Rect,
}

pub(super) struct CommandRegions {
    pub(super) overlay: Rect,
    pub(super) output: Rect,
}

pub(super) fn draw_action_menu(
    frame: &mut Frame<'_>,
    anchor: Rect,
    selection: usize,
) -> ActionMenuRegions {
    let width = 38.min(frame.area().width.saturating_sub(2));
    let height = ACTION_ITEMS.len() as u16 + 1;
    let minimum_x = frame.area().x.saturating_add(1);
    let maximum_x = frame
        .area()
        .right()
        .saturating_sub(width.saturating_add(1))
        .max(minimum_x);
    let x = anchor
        .right()
        .saturating_sub(width)
        .clamp(minimum_x, maximum_x);
    let below = anchor.y.saturating_add(1);
    let y = if below.saturating_add(height) <= frame.area().bottom() {
        below
    } else {
        anchor.y.saturating_sub(height)
    };
    let area = Rect::new(x, y, width, height);
    let list = Rect::new(area.x, area.y, area.width, ACTION_ITEMS.len() as u16);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().raised);

    let items = ACTION_ITEMS.iter().enumerate().map(|(index, action)| {
        let detail_width = UnicodeWidthStr::width(action.detail);
        let label = truncate_width(
            action.label,
            usize::from(list.width).saturating_sub(detail_width + 4),
        );
        let padding = usize::from(list.width)
            .saturating_sub(UnicodeWidthStr::width(label.as_str()) + detail_width + 3);
        let item = ListItem::new(Line::from(vec![
            Span::styled(
                if index == selection { " › " } else { "   " },
                Style::default().fg(palette().accent),
            ),
            Span::styled(
                label,
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(padding)),
            Span::styled(action.detail, Style::default().fg(palette().faint)),
        ]));
        if index == selection {
            item.style(Style::default().bg(palette().selected))
        } else {
            item
        }
    });
    frame.render_widget(List::new(items), list);
    frame.render_widget(
        Paragraph::new("Enter run   Esc close")
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette().muted)),
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
    );

    ActionMenuRegions {
        overlay: area,
        list,
    }
}

pub(super) fn draw_command(frame: &mut Frame<'_>, actions: &mut ActionsState) -> CommandRegions {
    let area = centered_min(frame.area(), 82, 68, 64, 18);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().panel);
    fill(
        frame,
        Rect::new(area.x, area.y, area.width, 3),
        palette().surface_alt,
    );
    fill(
        frame,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        palette().surface_alt,
    );

    let inner_x = area.x.saturating_add(2);
    let inner_width = area.width.saturating_sub(4);
    let (title, status, status_color) = match actions.status {
        CommandStatus::Input => ("GIT COMMAND", "NON-INTERACTIVE".to_owned(), palette().muted),
        CommandStatus::Running => ("COMMAND OUTPUT", "RUNNING".to_owned(), palette().yellow),
        CommandStatus::Complete {
            success: true,
            exit_code,
        } => (
            "COMMAND OUTPUT",
            format!("SUCCESS · exit {}", exit_code.unwrap_or(0)),
            palette().green,
        ),
        CommandStatus::Complete {
            success: false,
            exit_code,
        } => (
            "COMMAND OUTPUT",
            exit_code.map_or_else(
                || "FAILED".to_owned(),
                |code| format!("FAILED · exit {code}"),
            ),
            palette().red,
        ),
    };
    let title_padding = usize::from(inner_width)
        .saturating_sub(UnicodeWidthStr::width(title) + UnicodeWidthStr::width(status.as_str()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(title_padding)),
            Span::styled(
                status,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect::new(inner_x, area.y.saturating_add(1), inner_width, 1),
    );

    let command_area = Rect::new(inner_x, area.bottom().saturating_sub(5), inner_width, 3);
    let command_editable = actions.status != CommandStatus::Running;
    fill(
        frame,
        command_area,
        if command_editable {
            palette().selected
        } else {
            palette().raised
        },
    );
    if command_editable {
        fill(
            frame,
            Rect::new(command_area.x, command_area.y, 1, command_area.height),
            palette().accent,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            "COMMAND",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(
            command_area.x.saturating_add(2),
            command_area.y,
            command_area.width.saturating_sub(4),
            1,
        ),
    );
    let command = if command_editable {
        format!("git {}▌", actions.input)
    } else {
        actions.command.clone()
    };
    frame.render_widget(
        Paragraph::new(truncate_start_width(
            &command,
            usize::from(command_area.width.saturating_sub(4)),
        ))
        .style(Style::default().fg(palette().ink)),
        Rect::new(
            command_area.x.saturating_add(2),
            command_area.y.saturating_add(1),
            command_area.width.saturating_sub(4),
            1,
        ),
    );

    let output = Rect::new(
        inner_x,
        area.y.saturating_add(4),
        inner_width,
        command_area
            .y
            .saturating_sub(area.y.saturating_add(4))
            .saturating_sub(1),
    );
    let lines = command_lines(actions);
    let rendered_height = rendered_height(&lines, usize::from(output.width));
    actions.scroll_max = rendered_height
        .saturating_sub(usize::from(output.height))
        .min(usize::from(u16::MAX)) as u16;
    actions.scroll = actions.scroll.min(actions.scroll_max);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((actions.scroll, 0))
            .style(Style::default().fg(palette().ink)),
        output,
    );

    let footer = match actions.status {
        CommandStatus::Input => "Enter run   Ctrl+U clear   Esc close",
        CommandStatus::Running => "Running in background   Esc close",
        CommandStatus::Complete { .. } => {
            "Type next command   Enter run/re-run   ↑↓ scroll   Esc close"
        }
    };
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette().muted)),
        Rect::new(inner_x, area.bottom().saturating_sub(1), inner_width, 1),
    );

    CommandRegions {
        overlay: area,
        output,
    }
}

fn command_lines(actions: &ActionsState) -> Vec<Line<'static>> {
    if actions.status == CommandStatus::Input && actions.transcript.is_empty() {
        return if actions.stderr.is_empty() {
            vec![
                Line::styled(
                    "Run any non-interactive Git command from this repository.",
                    Style::default().fg(palette().ink),
                ),
                Line::raw(""),
                Line::styled(
                    "Examples: status --short · log --oneline -10 · remote -v",
                    Style::default().fg(palette().faint),
                ),
                Line::styled(
                    "Shell pipes and redirects are not interpreted.",
                    Style::default().fg(palette().faint),
                ),
            ]
        } else {
            vec![Line::styled(
                actions.stderr.clone(),
                Style::default().fg(palette().red),
            )]
        };
    }
    let mut lines = Vec::new();
    for (index, record) in actions.transcript.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let status = if record.success {
            format!("exit {}", record.exit_code.unwrap_or(0))
        } else {
            record
                .exit_code
                .map_or_else(|| "failed".to_owned(), |code| format!("exit {code}"))
        };
        lines.push(Line::from(vec![
            Span::styled(
                record.command.clone(),
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {status}"), Style::default().fg(palette().muted)),
        ]));
        if !record.stdout.is_empty() {
            lines.extend(
                record
                    .stdout
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(palette().ink))),
            );
        }
        if !record.stderr.is_empty() {
            lines.extend(
                record
                    .stderr
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(palette().red))),
            );
        }
        if record.stdout.is_empty() && record.stderr.is_empty() {
            lines.push(Line::styled(
                "Completed without output.",
                Style::default().fg(palette().faint),
            ));
        }
    }
    if actions.status == CommandStatus::Input && !actions.stderr.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            actions.stderr.clone(),
            Style::default().fg(palette().red),
        ));
    }
    if actions.status == CommandStatus::Running {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            "Waiting for Git...",
            Style::default().fg(palette().yellow),
        ));
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "Command completed without output.",
            Style::default().fg(palette().faint),
        ));
    }
    lines
}

fn rendered_height(lines: &[Line<'_>], width: usize) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| {
            let line_width: usize = line
                .spans
                .iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum();
            line_width.max(1).div_ceil(width)
        })
        .sum()
}

pub(super) fn draw_picker(frame: &mut Frame<'_>, picker: &mut RepositoryPicker) -> PickerRegions {
    let row_count = if picker.editing_path {
        picker.matches.len()
    } else {
        picker.entries.len()
    };
    let desired_height = (11 + row_count.min(11) as u16).clamp(14, 22);
    let area = centered_min(frame.area(), 82, 0, 56, desired_height);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().panel);
    fill(
        frame,
        Rect::new(area.x, area.y, area.width, 3),
        palette().surface_alt,
    );
    fill(
        frame,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        palette().surface_alt,
    );

    let inner_x = area.x.saturating_add(2);
    let inner_width = area.width.saturating_sub(4);
    let current_is_repo = picker.entries.first().is_some_and(|entry| entry.is_repo);
    let location_kind = if current_is_repo {
        "GIT REPOSITORY"
    } else {
        "DIRECTORY"
    };
    let title_width = "REPOSITORY  Switch working directory".len();
    let title_padding = usize::from(inner_width)
        .saturating_sub(title_width + UnicodeWidthStr::width(location_kind));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "REPOSITORY",
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Switch working directory",
                Style::default().fg(palette().faint),
            ),
            Span::raw(" ".repeat(title_padding)),
            Span::styled(
                location_kind,
                Style::default()
                    .fg(if current_is_repo {
                        palette().green
                    } else {
                        palette().muted
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect::new(inner_x, area.y.saturating_add(1), inner_width, 1),
    );

    let path_area = Rect::new(inner_x, area.y.saturating_add(4), inner_width, 3);
    fill(
        frame,
        path_area,
        if picker.editing_path {
            palette().selected
        } else {
            palette().raised
        },
    );
    if picker.editing_path {
        fill(
            frame,
            Rect::new(path_area.x, path_area.y, 1, path_area.height),
            palette().accent,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            "PATH",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(
            path_area.x.saturating_add(2),
            path_area.y,
            path_area.width.saturating_sub(4),
            1,
        ),
    );
    let path_text = truncate_start_width(
        &picker.path_input,
        usize::from(path_area.width.saturating_sub(4)),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                path_text,
                Style::default().fg(if picker.editing_path {
                    palette().ink
                } else {
                    palette().muted
                }),
            ),
            Span::styled(
                if picker.editing_path { "▌" } else { "" },
                Style::default().fg(palette().accent),
            ),
        ])),
        Rect::new(
            path_area.x.saturating_add(2),
            path_area.y.saturating_add(1),
            path_area.width.saturating_sub(4),
            1,
        ),
    );

    let section_title = if picker.editing_path {
        "MATCHES"
    } else {
        "BROWSE"
    };
    let section_detail = if !picker.editing_path && picker.loading {
        "loading…".to_owned()
    } else if picker.editing_path && picker.searching {
        "indexing…".to_owned()
    } else {
        format!("{} entries", row_count)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                section_title,
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {section_detail}"),
                Style::default().fg(palette().faint),
            ),
        ])),
        Rect::new(inner_x, area.y.saturating_add(8), inner_width, 1),
    );
    let list_y = area.y.saturating_add(10);
    let list_area = Rect::new(
        inner_x,
        list_y,
        inner_width,
        area.bottom().saturating_sub(1).saturating_sub(list_y),
    );
    if picker.editing_path {
        let items = picker
            .matches
            .iter()
            .map(|entry| picker_item(entry, usize::from(list_area.width)));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default().bg(palette().selected)),
            list_area,
            &mut picker.match_state,
        );
    } else {
        let items = picker
            .entries
            .iter()
            .map(|entry| picker_item(entry, usize::from(list_area.width)));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default().bg(palette().selected)),
            list_area,
            &mut picker.state,
        );
    }

    let footer = Rect::new(inner_x, area.bottom().saturating_sub(1), inner_width, 1);
    if let Some(error) = &picker.error {
        frame.render_widget(
            Paragraph::new(truncate_width(error, usize::from(footer.width)))
                .style(Style::default().fg(palette().red)),
            footer,
        );
    } else {
        let hint = if picker.editing_path {
            "Tab complete   Enter open   ↑↓ matches   Esc browse"
        } else {
            "Enter open   h parent   / search   Esc close"
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::default().fg(palette().muted))
                .alignment(Alignment::Right),
            footer,
        );
    }

    PickerRegions {
        overlay: area,
        path: path_area,
        list: list_area,
    }
}

fn picker_item(entry: &PickerEntry, width: usize) -> ListItem<'static> {
    let (marker, label, detail, color) = match entry.action {
        PickerAction::Open if entry.is_repo => ("● ", entry.label.clone(), "open", palette().green),
        PickerAction::Open => ("○ ", entry.label.clone(), "check", palette().muted),
        PickerAction::Navigate if entry.label == ".." => {
            ("↑ ", "Parent directory".to_owned(), "", palette().muted)
        }
        PickerAction::Navigate if entry.is_repo => {
            ("◆ ", entry.label.clone(), "repository", palette().green)
        }
        PickerAction::Navigate => ("› ", entry.label.clone(), "", palette().faint),
    };
    let detail_width = usize::from(!detail.is_empty()) + UnicodeWidthStr::width(detail);
    let label_width = width.saturating_sub(2 + detail_width);
    let label = truncate_width(&label, label_width);
    let padding = width.saturating_sub(2 + UnicodeWidthStr::width(label.as_str()) + detail_width);
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(color)),
        Span::styled(label, Style::default().fg(palette().ink)),
        Span::raw(" ".repeat(padding)),
    ];
    if !detail.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            detail.to_owned(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    ListItem::new(Line::from(spans))
}

pub(super) fn draw_settings(
    frame: &mut Frame<'_>,
    settings: &Settings,
    selection: usize,
    fetch_running: bool,
) -> SettingsRegions {
    let area = centered_min(frame.area(), 58, 0, 48, 17);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().panel);
    fill(
        frame,
        Rect::new(area.x, area.y, area.width, 3),
        palette().surface_alt,
    );
    fill(
        frame,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        palette().surface_alt,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "SETTINGS",
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Application preferences",
                Style::default().fg(palette().faint),
            ),
        ])),
        Rect::new(
            area.x.saturating_add(2),
            area.y.saturating_add(1),
            area.width.saturating_sub(4),
            1,
        ),
    );
    frame.render_widget(
        Paragraph::new("Space toggle   ←/→ interval   Enter edit   Esc close")
            .style(Style::default().fg(palette().muted))
            .alignment(Alignment::Right),
        Rect::new(
            area.x.saturating_add(2),
            area.bottom().saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        ),
    );

    let inner = Rect::new(
        area.x.saturating_add(2),
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );
    let auto_row = Rect::new(inner.x, area.y.saturating_add(7), inner.width, 1);
    let interval_row = Rect::new(inner.x, area.y.saturating_add(9), inner.width, 1);
    let editor_row = Rect::new(inner.x, area.y.saturating_add(14), inner.width, 1);
    let interval_down = Rect::new(
        interval_row.right().saturating_sub(15),
        interval_row.y,
        3,
        1,
    );
    let interval_up = Rect::new(interval_row.right().saturating_sub(3), interval_row.y, 3, 1);

    frame.render_widget(
        Paragraph::new(Line::styled(
            "AUTOMATION",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(inner.x, area.y.saturating_add(4), inner.width, 1),
    );
    let description = truncate_width(
        "Fetch updated remote refs in the background",
        usize::from(inner.width),
    );
    frame.render_widget(
        Paragraph::new(description).style(Style::default().fg(palette().faint)),
        Rect::new(inner.x, area.y.saturating_add(5), inner.width, 1),
    );

    let checkbox = if settings.auto_fetch { "◉" } else { "○" };
    let auto_padding =
        usize::from(auto_row.width).saturating_sub(19 + UnicodeWidthStr::width(checkbox));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Auto-fetch remotes", Style::default().fg(palette().ink)),
            Span::raw(" ".repeat(auto_padding)),
            Span::styled(
                format!("{checkbox} "),
                Style::default()
                    .fg(if settings.auto_fetch {
                        palette().green
                    } else {
                        palette().muted
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(if selection == 0 {
            palette().selected
        } else {
            palette().surface_alt
        })),
        auto_row,
    );

    let interval_control = format!("[-] {:>4} min [+]", settings.fetch_interval_minutes);
    let interval_padding = usize::from(interval_row.width)
        .saturating_sub("Fetch interval".len() + interval_control.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Fetch interval", Style::default().fg(palette().ink)),
            Span::raw(" ".repeat(interval_padding)),
            Span::styled(interval_control, Style::default().fg(palette().accent)),
        ]))
        .style(Style::default().bg(if selection == 1 {
            palette().selected
        } else {
            palette().surface_alt
        })),
        interval_row,
    );

    let status = if fetch_running {
        "Fetching remotes now...".to_owned()
    } else if settings.auto_fetch {
        format!(
            "Enabled · every {} minute{}",
            settings.fetch_interval_minutes,
            if settings.fetch_interval_minutes == 1 {
                ""
            } else {
                "s"
            }
        )
    } else {
        "Disabled".to_owned()
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(if settings.auto_fetch {
            palette().green
        } else {
            palette().faint
        })),
        Rect::new(inner.x, area.y.saturating_add(11), inner.width, 1),
    );

    frame.render_widget(
        Paragraph::new(Line::styled(
            "EDITOR",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(inner.x, area.y.saturating_add(13), inner.width, 1),
    );
    let editor = settings
        .editor_command
        .as_deref()
        .unwrap_or("Not configured");
    let editor = truncate_width(editor, usize::from(editor_row.width).saturating_sub(17));
    let editor_padding =
        usize::from(editor_row.width).saturating_sub("Editor command".len() + editor.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Editor command", Style::default().fg(palette().ink)),
            Span::raw(" ".repeat(editor_padding)),
            Span::styled(
                editor,
                Style::default().fg(if settings.editor_command.is_some() {
                    palette().accent
                } else {
                    palette().muted
                }),
            ),
        ]))
        .style(Style::default().bg(if selection == 2 {
            palette().selected
        } else {
            palette().surface_alt
        })),
        editor_row,
    );

    SettingsRegions {
        overlay: area,
        auto_fetch: auto_row,
        fetch_interval: interval_row,
        fetch_interval_down: interval_down,
        fetch_interval_up: interval_up,
        editor: editor_row,
    }
}

pub(super) fn draw_editor(
    frame: &mut Frame<'_>,
    input: &str,
    error: Option<&str>,
    configure_only: bool,
) -> Rect {
    let area = centered_min(frame.area(), 64, 0, 52, 12);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().panel);
    fill(
        frame,
        Rect::new(area.x, area.y, area.width, 3),
        palette().surface_alt,
    );
    fill(
        frame,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        palette().surface_alt,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "EDITOR COMMAND",
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Saved for next time",
                Style::default().fg(palette().faint),
            ),
        ])),
        Rect::new(
            area.x.saturating_add(2),
            area.y.saturating_add(1),
            area.width.saturating_sub(4),
            1,
        ),
    );
    let inner = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(4),
        area.width.saturating_sub(4),
        area.height.saturating_sub(5),
    );
    frame.render_widget(
        Paragraph::new("Choose the interactive editor used for selected files.")
            .style(Style::default().fg(palette().ink)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(format!("{input}▌"))
            .style(Style::default().fg(palette().ink).bg(palette().selected)),
        Rect::new(inner.x, inner.y.saturating_add(2), inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(error.unwrap_or("Examples: nvim · micro · code --wait")).style(
            Style::default().fg(if error.is_some() {
                palette().red
            } else {
                palette().faint
            }),
        ),
        Rect::new(inner.x, inner.y.saturating_add(4), inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(if configure_only {
            "Enter save   Ctrl+U clear   Esc back"
        } else {
            "Enter save & open   Ctrl+U clear   Esc cancel"
        })
        .style(Style::default().fg(palette().muted))
        .alignment(Alignment::Right),
        Rect::new(
            area.x.saturating_add(2),
            area.bottom().saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        ),
    );
    area
}

pub(super) fn draw_help(frame: &mut Frame<'_>) {
    let area = centered_min(frame.area(), 72, 0, 58, 17);
    frame.render_widget(Clear, area);
    fill(frame, area, palette().panel);
    fill(
        frame,
        Rect::new(area.x, area.y, area.width, 3),
        palette().surface_alt,
    );
    fill(
        frame,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        palette().surface_alt,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "KEYBOARD",
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Quick reference", Style::default().fg(palette().faint)),
        ])),
        Rect::new(
            area.x.saturating_add(2),
            area.y.saturating_add(1),
            area.width.saturating_sub(4),
            1,
        ),
    );
    let body = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(4),
        area.width.saturating_sub(4),
        area.height.saturating_sub(5),
    );
    let columns = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(2),
        Constraint::Percentage(50),
    ])
    .split(body);
    let navigation = vec![
        Line::styled(
            "NAVIGATION",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        ),
        help_line("1 / 2 / Tab", "Switch view"),
        help_line("j / k", "Move / scroll hunk ×10"),
        help_line("Home / G", "First / last"),
        help_line("r", "Refresh"),
        help_line("o", "Repository"),
        help_line("s", "Settings"),
        help_line("x", "Git actions"),
        help_line("g", "Git command"),
        help_line("e / E", "Edit / configure editor"),
        help_line("f", "Worktree / files"),
        help_line("w", "Wrap diff"),
    ];
    let worktree = vec![
        Line::styled(
            "WORKTREE",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        ),
        help_line("← / h", "Tree / exit hunk"),
        help_line("→ / l", "Enter / stage hunk"),
        help_line("Enter", "Toggle folder"),
        help_line("Space", "Stage file / hunk"),
        help_line("a / u", "Stage / unstage all"),
        help_line("c", "Commit editor"),
        help_line("← / →", "Commit cursor"),
        help_line("C-A / C-⌫", "Select all / del word"),
        help_line("Ctrl+Enter", "Commit"),
        help_line("Esc", "Close / unfocus"),
        help_line("q", "Quit"),
    ];
    frame.render_widget(Paragraph::new(navigation), columns[0]);
    frame.render_widget(Paragraph::new(worktree), columns[2]);
    frame.render_widget(
        Paragraph::new("? / Esc close")
            .style(Style::default().fg(palette().muted))
            .alignment(Alignment::Right),
        Rect::new(
            area.x.saturating_add(2),
            area.bottom().saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        ),
    );
}

fn truncate_start_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }

    let target = width.saturating_sub(1);
    let mut suffix = String::new();
    let mut used = 0;
    for character in value.chars().rev() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > target {
            break;
        }
        suffix.insert(0, character);
        used += character_width;
    }
    format!("…{suffix}")
}

fn help_line<'a>(key: &'a str, description: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!(" {key:<12}"),
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(description, Style::default().fg(palette().ink)),
    ])
}

fn centered_min(
    area: Rect,
    width_percent: u16,
    height_percent: u16,
    minimum_width: u16,
    minimum_height: u16,
) -> Rect {
    let width = area
        .width
        .saturating_mul(width_percent)
        .checked_div(100)
        .unwrap_or(0)
        .max(minimum_width)
        .min(area.width.saturating_sub(4));
    let height = area
        .height
        .saturating_mul(height_percent)
        .checked_div(100)
        .unwrap_or(0)
        .max(minimum_height)
        .min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
