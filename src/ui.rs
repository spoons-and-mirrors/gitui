use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Mode, PickerAction, Regions, View, WorktreeRow},
    git::{Change, Commit},
    theme::{Palette, load_theme},
};

fn palette() -> &'static Palette {
    static THEME: std::sync::OnceLock<Palette> = std::sync::OnceLock::new();
    THEME.get_or_init(|| load_theme().palette)
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().canvas).fg(palette().ink)),
        frame.area(),
    );

    if frame.area().width < 60 || frame.area().height < 16 {
        frame.render_widget(
            Paragraph::new("Git Panel needs at least 60 columns and 16 rows\n\nq  quit")
                .alignment(Alignment::Center)
                .style(Style::default().fg(palette().ink)),
            frame.area(),
        );
        return;
    }

    let layout = Layout::vertical([Constraint::Length(3), Constraint::Min(6)]).split(frame.area());

    draw_header(frame, app, layout[0]);
    let content = layout[1];
    match app.view {
        View::Changes => draw_changes(frame, app, content),
        View::Graph => draw_graph(frame, app, content),
    }
    match app.mode {
        Mode::Picker => draw_picker(frame, app),
        Mode::Settings => draw_settings(frame, app),
        Mode::Help => draw_help(frame),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (path, branch) = app.repo.as_ref().map_or_else(
        || ("No repository selected".to_owned(), "offline".to_owned()),
        |repo| (repo.root.display().to_string(), repo.branch.clone()),
    );
    let (staged, unstaged) = app.change_counts();
    let commits = app.repo.as_ref().map_or(0, |repo| repo.commits.len());

    frame.render_widget(
        Block::default().style(Style::default().bg(palette().panel)),
        Rect::new(area.x, area.y, area.width, 2),
    );
    let repository = std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gitui");
    let branch_label = format!("  {branch} ");
    let fixed_width = UnicodeWidthStr::width(repository)
        .saturating_add(UnicodeWidthStr::width(branch_label.as_str()))
        .saturating_add(6);
    let display_path = truncate_width(&path, usize::from(area.width).saturating_sub(fixed_width));
    let mut title = vec![
        Span::styled(
            format!("  {repository}"),
            Style::default()
                .fg(palette().ink)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {display_path}"),
            Style::default().fg(palette().faint),
        ),
        Span::styled(
            branch_label,
            Style::default()
                .fg(palette().accent)
                .bg(palette().surface_alt)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(notice) = &app.notice {
        title.push(Span::styled(
            format!("  {notice}"),
            Style::default().fg(palette().yellow),
        ));
    }

    let changes_label = format!(" 1 Changes {}/{} ", staged, unstaged);
    let graph_label = format!(" 2 Graph {commits} ");
    let compact = area.width < 72;
    let refresh_label = if compact { " r " } else { " r Refresh " };
    let repository_label = if compact { " o " } else { " o Repository " };
    let settings_label = if compact { " s " } else { " s Settings " };
    let help_label = if compact { " ? " } else { " ? Help " };
    let labels = [
        changes_label.as_str(),
        graph_label.as_str(),
        refresh_label,
        repository_label,
        settings_label,
        help_label,
    ];

    let mut spans = Vec::new();
    let mut x = area.x;
    let mut rects = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let active =
            (index == 0 && app.view == View::Changes) || (index == 1 && app.view == View::Graph);
        spans.push(Span::styled(
            *label,
            if active {
                Style::default()
                    .fg(palette().accent)
                    .bg(palette().raised)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().muted)
            },
        ));
        let width = UnicodeWidthStr::width(*label) as u16;
        rects.push(Rect::new(x, area.y + 1, width, 1));
        x = x.saturating_add(width);
    }

    app.regions = Regions {
        changes: rects.first().copied(),
        graph: rects.get(1).copied(),
        refresh: rects.get(2).copied(),
        repository: rects.get(3).copied(),
        settings: rects.get(4).copied(),
        help: rects.get(5).copied(),
        ..Regions::default()
    };

    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(title), Line::from(spans)])),
        Rect::new(area.x, area.y, area.width, 2),
    );
}

fn draw_changes(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.repo.is_none() {
        draw_empty(frame, area, "Open a repository to inspect its worktree");
        return;
    }

    let left_width = app
        .settings
        .worktree_width
        .clamp(24, area.width.saturating_sub(25));
    let columns = [
        Rect::new(area.x, area.y, left_width, area.height),
        Rect::new(
            area.x.saturating_add(left_width).saturating_add(1),
            area.y,
            area.width.saturating_sub(left_width).saturating_sub(1),
            area.height,
        ),
    ];
    app.regions.worktree = Some(columns[0]);
    app.regions.diff = Some(columns[1]);
    app.regions.split_bounds = Some(area);
    app.regions.splitter = Some(Rect::new(columns[0].right(), area.y, 1, area.height));
    fill(frame, columns[0], palette().panel);
    fill(frame, columns[1], palette().panel);
    if app.dragging_splitter {
        fill(
            frame,
            Rect::new(columns[0].right(), area.y, 1, area.height),
            palette().accent,
        );
    }

    let worktree_content = columns[0].inner(Margin::new(1, 0));
    let commit_area = Rect::new(
        worktree_content.x,
        worktree_content.bottom().saturating_sub(5),
        worktree_content.width,
        5,
    );
    app.regions.commit = Some(commit_area);

    let repo = app.repo.as_ref().expect("checked above");
    let staged_count = repo.changes.iter().filter(|change| change.staged).count();
    let checkbox = if !repo.changes.is_empty() && staged_count == repo.changes.len() {
        "[x]"
    } else if staged_count > 0 {
        "[-]"
    } else {
        "[ ]"
    };
    let checkbox_color = if staged_count == repo.changes.len() && staged_count > 0 {
        palette().green
    } else if staged_count > 0 {
        palette().yellow
    } else {
        palette().muted
    };
    let worktree_header = Rect::new(
        worktree_content.x,
        worktree_content.y.saturating_add(1),
        worktree_content.width,
        1,
    );
    let worktree_list = Rect::new(
        worktree_header.x,
        worktree_header.y.saturating_add(2),
        worktree_header.width,
        commit_area
            .y
            .saturating_sub(worktree_header.y.saturating_add(2)),
    );
    app.regions.worktree_list = Some(worktree_list);
    app.regions.worktree_status = Some(Rect::new(
        worktree_list.right().saturating_sub(3),
        worktree_list.y,
        worktree_list.width.min(3),
        worktree_list.height,
    ));
    app.regions.stage_all = Some(Rect::new(
        worktree_header.right().saturating_sub(3),
        worktree_header.y,
        3,
        1,
    ));
    app.regions.unstage_all = None;

    let worktree_rows = app.worktree_rows();
    let items: Vec<ListItem<'_>> = worktree_rows
        .iter()
        .map(|row| worktree_item(row, &repo.changes, worktree_list.width as usize))
        .collect();
    let list = List::new(items).highlight_style(Style::default().bg(if app.mode == Mode::Commit {
        palette().inactive_selected
    } else {
        palette().selected
    }));
    let stage_label = if worktree_header.width >= 36 {
        format!("Stage all  {} files", repo.changes.len())
    } else {
        "All".to_owned()
    };
    let worktree_title = if worktree_header.width >= 30 {
        format!("WORKTREE  {}", repo.changes.len())
    } else {
        "WORKTREE".to_owned()
    };
    let title_width = UnicodeWidthStr::width(worktree_title.as_str());
    let stage_width = UnicodeWidthStr::width(stage_label.as_str()) + 4;
    let stage_padding =
        usize::from(worktree_header.width).saturating_sub(title_width + stage_width);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                worktree_title,
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(stage_padding)),
            Span::styled(
                format!("{stage_label} "),
                Style::default().fg(palette().muted),
            ),
            Span::styled(
                checkbox,
                Style::default()
                    .fg(checkbox_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        worktree_header,
    );
    frame.render_stateful_widget(list, worktree_list, &mut app.changes_state);

    let selected_change = app
        .changes_state
        .selected()
        .and_then(|index| worktree_rows.get(index))
        .and_then(|row| row.change_index)
        .and_then(|index| repo.changes.get(index));
    let selected_path = selected_change.map_or("No file selected", |change| change.path.as_str());
    let diff_header = Rect::new(
        columns[1].x.saturating_add(1),
        columns[1].y.saturating_add(1),
        columns[1].width.saturating_sub(2),
        1,
    );
    let diff_body = Rect::new(
        diff_header.x,
        diff_header.y.saturating_add(2),
        diff_header.width,
        columns[1]
            .bottom()
            .saturating_sub(diff_header.y.saturating_add(3)),
    );
    let state = selected_change.map_or(
        "",
        |change| {
            if change.staged { "staged" } else { "unstaged" }
        },
    );
    let wrap_label = if app.diff_wrap { "  w:on" } else { "  w:off" };
    let display_path = truncate_width(
        selected_path,
        usize::from(diff_header.width)
            .saturating_sub(8 + UnicodeWidthStr::width(state) + UnicodeWidthStr::width(wrap_label)),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "DIFF  ",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                display_path,
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {state}"),
                Style::default().fg(if state == "staged" {
                    palette().green
                } else {
                    palette().yellow
                }),
            ),
            Span::styled(
                wrap_label,
                Style::default().fg(if app.diff_wrap {
                    palette().accent
                } else {
                    palette().faint
                }),
            ),
        ])),
        diff_header,
    );
    let diff_lines = styled_diff(&app.diff, selected_path, usize::from(diff_body.width));
    let mut diff = Paragraph::new(diff_lines)
        .scroll((app.diff_scroll, 0))
        .style(Style::default().bg(palette().panel));
    if app.diff_wrap {
        diff = diff.wrap(Wrap { trim: false });
    }
    frame.render_widget(diff, diff_body);

    let commit_active = app.mode == Mode::Commit;
    fill(frame, commit_area, palette().canvas);
    if commit_active {
        fill(
            frame,
            Rect::new(commit_area.x, commit_area.y, 1, commit_area.height),
            palette().accent,
        );
    }
    let commit_content = commit_area.inner(Margin::new(1, 0));
    let commit_text = if app.commit_running {
        Text::from(Line::styled(
            "Creating commit...",
            Style::default().fg(palette().yellow),
        ))
    } else if commit_active || !app.commit_message.is_empty() {
        let mut lines: Vec<Line<'_>> = app
            .commit_message
            .split('\n')
            .map(|line| {
                Line::styled(
                    line,
                    Style::default().fg(if commit_active {
                        palette().ink
                    } else {
                        palette().muted
                    }),
                )
            })
            .collect();
        if commit_active && let Some(last) = lines.last_mut() {
            last.spans
                .push(Span::styled("█", Style::default().fg(palette().accent)));
        }
        Text::from(lines)
    } else {
        let hint = "Ctrl+Enter commit";
        let placeholder = "Write a commit message";
        if commit_content.width >= 40 {
            let padding =
                usize::from(commit_content.width).saturating_sub(placeholder.len() + hint.len());
            Text::from(Line::from(vec![
                Span::styled(placeholder, Style::default().fg(palette().muted)),
                Span::raw(" ".repeat(padding)),
                Span::styled(hint, Style::default().fg(palette().faint)),
            ]))
        } else {
            Text::from(Line::styled(
                placeholder,
                Style::default().fg(palette().muted),
            ))
        }
    };
    let commit_scroll = app.commit_message.split('\n').count().saturating_sub(5) as u16;
    frame.render_widget(
        Paragraph::new(commit_text)
            .scroll((commit_scroll, 0))
            .style(Style::default().bg(palette().canvas)),
        commit_content,
    );
}

fn draw_graph(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let Some(repo) = &app.repo else {
        draw_empty(frame, area, "Open a repository to inspect its graph");
        return;
    };
    if repo.commits.is_empty() {
        draw_empty(frame, area, "This repository has no commits yet");
        return;
    }
    fill(frame, area, palette().panel);
    let graph_header = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "ALL BRANCHES",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  date order", Style::default().fg(palette().faint)),
        ])),
        graph_header,
    );
    let table_area = Rect::new(
        graph_header.x,
        graph_header.y.saturating_add(2),
        graph_header.width,
        area.bottom()
            .saturating_sub(graph_header.y.saturating_add(3)),
    );
    app.regions.graph_table = Some(Rect::new(
        table_area.x,
        table_area.y.saturating_add(2),
        table_area.width,
        table_area.height.saturating_sub(2),
    ));

    let maximum_graph_width = table_area.width.saturating_sub(35).clamp(8, 40);
    let graph_width = repo
        .commits
        .iter()
        .map(|commit| commit.graph.len())
        .max()
        .unwrap_or(1)
        .clamp(8, maximum_graph_width as usize) as u16;
    let compact = table_area.width < 110;
    let widths = if compact {
        vec![
            Constraint::Length(graph_width),
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(9),
        ]
    } else {
        vec![
            Constraint::Length(graph_width),
            Constraint::Min(30),
            Constraint::Length(12),
            Constraint::Length(16),
            Constraint::Length(9),
        ]
    };

    let rows = repo.commits.iter().map(|commit| graph_row(commit, compact));
    let headers = if compact {
        Row::new(["GRAPH", "DESCRIPTION", "AUTHOR", "COMMIT"])
    } else {
        Row::new(["GRAPH", "DESCRIPTION", "DATE", "AUTHOR", "COMMIT"])
    }
    .style(
        Style::default()
            .fg(palette().muted)
            .bg(palette().surface_alt)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let table = Table::new(rows, widths)
        .header(headers)
        .column_spacing(1)
        .row_highlight_style(Style::default().bg(palette().selected));
    frame.render_stateful_widget(table, table_area, &mut app.graph_state);
}

fn graph_row(commit: &Commit, compact: bool) -> Row<'static> {
    let graph = Line::from(
        commit
            .graph
            .iter()
            .map(|cell| {
                Span::styled(
                    cell.symbol.to_string(),
                    Style::default()
                        .fg(palette().graph_colors[cell.color % palette().graph_colors.len()]),
                )
            })
            .collect::<Vec<_>>(),
    );

    let mut description = Vec::new();
    if commit
        .refs
        .iter()
        .any(|reference| reference == "HEAD" || reference.starts_with("HEAD -> "))
    {
        description.push(ref_badge("HEAD", palette().green));
        description.push(Span::raw(" "));
    }
    for reference in &commit.refs {
        let (label, color) = if let Some(tag) = reference.strip_prefix("tag: ") {
            (tag, palette().yellow)
        } else if let Some(branch) = reference.strip_prefix("HEAD -> ") {
            (branch, palette().accent)
        } else if reference == "HEAD" {
            continue;
        } else {
            (reference.as_str(), palette().accent)
        };
        description.push(ref_badge(label, color));
        description.push(Span::raw(" "));
    }
    description.push(Span::styled(
        commit.subject.clone(),
        Style::default().fg(palette().ink),
    ));

    let short_oid: String = commit.oid.chars().take(7).collect();
    if compact {
        Row::new([
            Cell::from(graph),
            Cell::from(Line::from(description)),
            Cell::from(commit.author.clone()).style(Style::default().fg(palette().muted)),
            Cell::from(short_oid).style(Style::default().fg(palette().muted)),
        ])
    } else {
        Row::new([
            Cell::from(graph),
            Cell::from(Line::from(description)),
            Cell::from(commit.date.clone()).style(Style::default().fg(palette().muted)),
            Cell::from(commit.author.clone()).style(Style::default().fg(palette().muted)),
            Cell::from(short_oid).style(Style::default().fg(palette().muted)),
        ])
    }
}

fn ref_badge(label: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(color)
            .bg(palette().raised)
            .add_modifier(Modifier::BOLD),
    )
}

fn draw_picker(frame: &mut Frame<'_>, app: &mut App) {
    let area = centered(frame.area(), 76, 78);
    app.regions.picker_overlay = Some(area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .title(Span::styled(
                " OPEN REPOSITORY ",
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_bottom(
                Line::from(if app.picker.editing_path {
                    " enter open path  ctrl-u clear  esc browse "
                } else {
                    " enter open  p type path  h parent  esc cancel "
                })
                .alignment(Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().accent))
            .style(Style::default().bg(palette().raised)),
        area,
    );
    let inner = area.inner(Margin::new(2, 1));
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
    ])
    .split(inner);
    app.regions.picker_path = Some(parts[0]);
    app.regions.picker_list = Some(parts[2]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "PATH      ",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                &app.picker.path_input,
                Style::default().fg(if app.picker.editing_path {
                    palette().ink
                } else {
                    palette().muted
                }),
            ),
            Span::styled(
                if app.picker.editing_path { "▌" } else { "" },
                Style::default().fg(palette().accent),
            ),
        ])),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "LOCATION  ",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.picker.directory.display().to_string(),
                Style::default().fg(palette().ink),
            ),
        ])),
        parts[1],
    );
    let items = app.picker.entries.iter().map(|entry| {
        let marker = match entry.action {
            PickerAction::Open if entry.is_repo => "● ",
            PickerAction::Open => "○ ",
            PickerAction::Navigate if entry.is_repo => "◆ ",
            PickerAction::Navigate => "  ",
        };
        let color = if entry.is_repo {
            palette().green
        } else {
            palette().muted
        };
        ListItem::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(color)),
            Span::styled(entry.label.clone(), Style::default().fg(palette().ink)),
        ]))
    });
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::default().bg(palette().selected))
            .highlight_symbol("› "),
        parts[2],
        &mut app.picker.state,
    );
    if let Some(error) = &app.picker.error {
        frame.render_widget(
            Paragraph::new(error.as_str()).style(Style::default().fg(palette().red)),
            parts[3],
        );
    }
}

fn draw_settings(frame: &mut Frame<'_>, app: &mut App) {
    let area = centered_min(frame.area(), 58, 0, 48, 14);
    app.regions.settings_overlay = Some(area);
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
                "  Repository preferences",
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
        Paragraph::new("Space toggle   ←/→ interval   Esc close")
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
    app.regions.auto_fetch = Some(auto_row);
    app.regions.fetch_interval = Some(interval_row);
    app.regions.fetch_interval_down = Some(Rect::new(
        interval_row.right().saturating_sub(15),
        interval_row.y,
        3,
        1,
    ));
    app.regions.fetch_interval_up = Some(Rect::new(
        interval_row.right().saturating_sub(3),
        interval_row.y,
        3,
        1,
    ));

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

    let checkbox = if app.settings.auto_fetch {
        "[x]"
    } else {
        "[ ]"
    };
    let auto_padding = usize::from(auto_row.width).saturating_sub(18 + checkbox.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Auto-fetch remotes", Style::default().fg(palette().ink)),
            Span::raw(" ".repeat(auto_padding)),
            Span::styled(
                checkbox,
                Style::default()
                    .fg(if app.settings.auto_fetch {
                        palette().green
                    } else {
                        palette().muted
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(if app.settings_selection == 0 {
            palette().selected
        } else {
            palette().surface_alt
        })),
        auto_row,
    );

    let interval_control = format!("[-] {:>4} min [+]", app.settings.fetch_interval_minutes);
    let interval_padding = usize::from(interval_row.width)
        .saturating_sub("Fetch interval".len() + interval_control.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Fetch interval", Style::default().fg(palette().ink)),
            Span::raw(" ".repeat(interval_padding)),
            Span::styled(interval_control, Style::default().fg(palette().accent)),
        ]))
        .style(Style::default().bg(if app.settings_selection == 1 {
            palette().selected
        } else {
            palette().surface_alt
        })),
        interval_row,
    );

    let status = if app.fetch_running {
        "Fetching remotes now...".to_owned()
    } else if app.settings.auto_fetch {
        format!(
            "Enabled · every {} minute{}",
            app.settings.fetch_interval_minutes,
            if app.settings.fetch_interval_minutes == 1 {
                ""
            } else {
                "s"
            }
        )
    } else {
        "Disabled".to_owned()
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(if app.settings.auto_fetch {
            palette().green
        } else {
            palette().faint
        })),
        Rect::new(inner.x, area.y.saturating_add(11), inner.width, 1),
    );
}

fn draw_help(frame: &mut Frame<'_>) {
    let area = centered_min(frame.area(), 72, 0, 58, 14);
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
        help_line("j / k", "Move"),
        help_line("g / G", "First / last"),
        help_line("r", "Refresh"),
        help_line("o", "Repository"),
        help_line("s", "Settings"),
        help_line("w", "Wrap diff"),
    ];
    let worktree = vec![
        Line::styled(
            "WORKTREE",
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        ),
        help_line("h / l", "Collapse / expand"),
        help_line("Enter", "Toggle folder"),
        help_line("Space", "Stage file"),
        help_line("a / u", "Stage / unstage all"),
        help_line("c", "Commit editor"),
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

fn worktree_item<'a>(row: &'a WorktreeRow, changes: &'a [Change], width: usize) -> ListItem<'a> {
    let Some(change_index) = row.change_index else {
        let marker = if row.directory_expanded == Some(false) {
            "▸ "
        } else {
            "▾ "
        };
        let directory = truncate_width(&format!("{}{}{}", row.prefix, marker, row.label), width);
        return ListItem::new(Line::from(Span::styled(
            directory,
            Style::default().fg(palette().muted),
        )));
    };
    let change = &changes[change_index];
    let (checkbox, color) = if change.staged {
        ("[x]", palette().green)
    } else {
        ("[ ]", palette().muted)
    };
    let label = change.original_path.as_ref().map_or_else(
        || row.label.clone(),
        |original| {
            let original_name = original.rsplit('/').next().unwrap_or(original);
            format!("{original_name} → {}", row.label)
        },
    );
    let available_label = width.saturating_sub(3);
    let path = truncate_width(&format!("{}{}", row.prefix, label), available_label);
    let padding = available_label.saturating_sub(UnicodeWidthStr::width(path.as_str()));
    ListItem::new(Line::from(vec![
        Span::styled(path, Style::default().fg(palette().ink)),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            checkbox,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]))
}

fn truncate_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }

    let target = width.saturating_sub(1);
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > target {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

fn styled_diff(diff: &str, path: &str, width: usize) -> Vec<Line<'static>> {
    let numbered = width >= 72;
    let mut old_line = None;
    let mut new_line = None;

    diff.lines()
        .map(|line| {
            if line.starts_with("@@") {
                if let Some((old, new)) = parse_hunk_lines(line) {
                    old_line = Some(old);
                    new_line = Some(new);
                }
                return finish_diff_line(
                    vec![Span::styled(
                        line.to_owned(),
                        Style::default()
                            .fg(palette().cyan)
                            .add_modifier(Modifier::BOLD),
                    )],
                    width,
                    palette().surface_alt,
                );
            }
            if line.starts_with("diff --git") {
                return finish_diff_line(
                    vec![Span::styled(
                        line.to_owned(),
                        Style::default()
                            .fg(palette().accent)
                            .add_modifier(Modifier::BOLD),
                    )],
                    width,
                    palette().panel,
                );
            }
            if line.starts_with("index ") {
                return finish_diff_line(
                    vec![Span::styled(
                        line.to_owned(),
                        Style::default().fg(palette().faint),
                    )],
                    width,
                    palette().panel,
                );
            }
            if line.starts_with("---") || line.starts_with("+++") {
                let color = if line.starts_with("---") {
                    palette().red
                } else {
                    palette().green
                };
                return finish_diff_line(
                    vec![Span::styled(line.to_owned(), Style::default().fg(color))],
                    width,
                    palette().panel,
                );
            }
            if line.starts_with("\\ No newline") {
                return finish_diff_line(
                    vec![Span::styled(
                        line.to_owned(),
                        Style::default().fg(palette().yellow),
                    )],
                    width,
                    palette().panel,
                );
            }
            if line.starts_with("Untracked file:") || line.starts_with("Binary untracked file") {
                return finish_diff_line(
                    vec![Span::styled(
                        line.to_owned(),
                        Style::default()
                            .fg(palette().yellow)
                            .add_modifier(Modifier::BOLD),
                    )],
                    width,
                    palette().panel,
                );
            }

            let (marker, payload, background, old_number, new_number) =
                if let Some(payload) = line.strip_prefix('+') {
                    let number = new_line;
                    new_line = new_line.map(|value| value + 1);
                    ("+", payload, palette().add_bg, None, number)
                } else if let Some(payload) = line.strip_prefix('-') {
                    let number = old_line;
                    old_line = old_line.map(|value| value + 1);
                    ("-", payload, palette().remove_bg, number, None)
                } else if let Some(payload) = line.strip_prefix(' ')
                    && old_line.is_some()
                {
                    let old = old_line;
                    let new = new_line;
                    old_line = old_line.map(|value| value + 1);
                    new_line = new_line.map(|value| value + 1);
                    (" ", payload, palette().panel, old, new)
                } else {
                    return finish_diff_line(syntax_spans(line, path), width, palette().panel);
                };

            let mut spans = if numbered {
                diff_line_numbers(old_number, new_number)
            } else {
                Vec::new()
            };
            spans.push(Span::styled(
                marker.to_owned(),
                Style::default()
                    .fg(if marker == "+" {
                        palette().green
                    } else if marker == "-" {
                        palette().red
                    } else {
                        palette().faint
                    })
                    .add_modifier(Modifier::BOLD),
            ));
            spans.extend(syntax_spans(payload, path));
            finish_diff_line(spans, width, background)
        })
        .collect()
}

fn parse_hunk_lines(line: &str) -> Option<(u32, u32)> {
    let mut fields = line.split_whitespace();
    fields.next()?;
    let old = fields
        .next()?
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = fields
        .next()?
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

fn diff_line_numbers(old: Option<u32>, new: Option<u32>) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!(
            "{:>4} {:>4} ",
            old.map_or_else(String::new, |value| value.to_string()),
            new.map_or_else(String::new, |value| value.to_string())
        ),
        Style::default().fg(palette().faint),
    )]
}

fn finish_diff_line(
    mut spans: Vec<Span<'static>>,
    width: usize,
    background: Color,
) -> Line<'static> {
    let used: usize = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    Line::from(spans).style(Style::default().bg(background))
}

fn syntax_spans(code: &str, path: &str) -> Vec<Span<'static>> {
    let hash_comments = matches!(
        path.rsplit('.').next().unwrap_or_default(),
        "py" | "rb" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml"
    );
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < code.len() {
        let rest = &code[cursor..];
        if rest.starts_with("//") || (hash_comments && rest.starts_with('#')) {
            spans.push(Span::styled(
                rest.to_owned(),
                Style::default().fg(palette().faint),
            ));
            break;
        }
        let character = rest.chars().next().expect("nonempty remainder");
        if character == '"' || character == '\'' {
            let mut escaped = false;
            let mut end = character.len_utf8();
            for next in rest[character.len_utf8()..].chars() {
                end += next.len_utf8();
                if next == character && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
                if next != '\\' {
                    escaped = false;
                }
            }
            spans.push(Span::styled(
                rest[..end].to_owned(),
                Style::default().fg(palette().yellow),
            ));
            cursor += end;
            continue;
        }
        if character.is_alphanumeric() || character == '_' {
            let end = rest
                .char_indices()
                .find_map(|(index, next)| {
                    (!(next.is_alphanumeric() || next == '_')).then_some(index)
                })
                .unwrap_or(rest.len());
            let token = &rest[..end];
            let following = rest[end..].trim_start();
            let color = if is_keyword(token) {
                palette().purple
            } else if token.chars().all(|next| next.is_ascii_digit()) {
                palette().orange
            } else if token.chars().next().is_some_and(char::is_uppercase)
                || following.starts_with('(')
            {
                palette().cyan
            } else {
                palette().ink
            };
            spans.push(Span::styled(token.to_owned(), Style::default().fg(color)));
            cursor += end;
            continue;
        }
        let (token, color) =
            if rest.starts_with("::") || rest.starts_with("->") || rest.starts_with("=>") {
                (&rest[..2], palette().cyan)
            } else {
                (&rest[..character.len_utf8()], palette().ink)
            };
        spans.push(Span::styled(token.to_owned(), Style::default().fg(color)));
        cursor += token.len();
    }
    spans
}

fn is_keyword(token: &str) -> bool {
    matches!(
        token,
        "as" | "async"
            | "await"
            | "break"
            | "class"
            | "const"
            | "continue"
            | "crate"
            | "def"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "from"
            | "function"
            | "if"
            | "impl"
            | "import"
            | "in"
            | "interface"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "new"
            | "none"
            | "null"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "throw"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "use"
            | "var"
            | "where"
            | "while"
            | "yield"
    )
}

fn draw_empty(frame: &mut Frame<'_>, area: Rect, message: &str) {
    fill(frame, area, palette().panel);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                message,
                Style::default()
                    .fg(palette().ink)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                "Press o to choose a directory",
                Style::default().fg(palette().muted),
            ),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

fn fill(frame: &mut Frame<'_>, area: Rect, color: Color) {
    frame.render_widget(Block::default().style(Style::default().bg(color)), area);
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

fn centered(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1])[1]
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

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn renders_every_primary_surface() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        run_git(root, &["init", "-b", "main"]);
        run_git(root, &["config", "user.name", "Render Test"]);
        run_git(root, &["config", "user.email", "render@example.com"]);
        fs::write(root.join("tracked.txt"), "first\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "initial commit"]);
        fs::write(root.join("second.txt"), "second\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "second commit"]);
        fs::write(root.join("tracked.txt"), "changed\n").unwrap();
        fs::write(root.join("untracked.txt"), "new\n").unwrap();

        let mut app = App::new(root.to_path_buf());
        let settings_path = root.join(".git/gitui-test-config");
        app.settings_path = Some(settings_path.clone());
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.regions.worktree.unwrap().x, 0);
        assert_eq!(app.regions.diff.unwrap().right(), 120);

        let stage_all = app.regions.stage_all.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            stage_all.x,
            stage_all.y,
        ));
        assert!(
            app.repo
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .all(|change| change.staged)
        );
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let stage_all = app.regions.stage_all.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            stage_all.x,
            stage_all.y,
        ));
        assert!(
            app.repo
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .all(|change| !change.staged)
        );

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let status = app.regions.worktree_status.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            status.x,
            status.y,
        ));
        assert_eq!(
            app.repo
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .filter(|change| change.staged)
                .count(),
            1
        );
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let status = app.regions.worktree_status.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            status.x,
            status.y,
        ));
        assert!(
            app.repo
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .all(|change| !change.staged)
        );

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let worktree = app.regions.worktree_list.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            worktree.x + 10,
            worktree.y + 1,
        ));
        assert_eq!(app.changes_state.selected(), Some(1));

        let splitter = app.regions.splitter.unwrap();
        let bounds = app.regions.split_bounds.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            splitter.x,
            splitter.y + 2,
        ));
        let target = bounds.x + 65;
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            target,
            splitter.y + 2,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            target,
            splitter.y + 2,
        ));
        assert_eq!(app.settings.worktree_width, 65);
        assert!(
            fs::read_to_string(settings_path)
                .unwrap()
                .contains("worktree_width=65")
        );
        assert!(!app.dragging_splitter);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let changes_screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(changes_screen.contains("Write a commit message"));
        assert!(!changes_screen.contains("[Commit]"));
        assert!(!changes_screen.contains("COMMIT"));
        assert!(!changes_screen.contains('┌'));
        let commit = app.regions.commit.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            commit.x + 2,
            commit.y + 1,
        ));
        assert_eq!(app.mode, Mode::Commit);
        app.commit_message = "Subject".to_owned();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.commit_message, "Subject\n");
        app.commit_message.push_str("Body");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Normal);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let unfocused_screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(unfocused_screen.contains("Subject"));
        assert!(unfocused_screen.contains("Body"));

        app.mode = Mode::Commit;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let diff = app.regions.diff.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            diff.x + 1,
            diff.y + 1,
        ));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.commit_message, "Subject\nBody");

        app.mode = Mode::Commit;
        app.commit_message.clear();
        app.notice = None;
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(
            app.notice.as_deref(),
            Some("Commit message cannot be empty")
        );

        app.view = View::Graph;
        app.mode = Mode::Normal;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("AUTHOR"));
        assert!(screen.contains("HEAD"));
        assert!(screen.contains("Render Test"));
        let graph = app.regions.graph_table.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            graph.x + 1,
            graph.y + 1,
        ));
        assert_eq!(app.graph_state.selected(), Some(1));

        app.mode = Mode::Picker;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let path = app.regions.picker_path.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            path.x,
            path.y,
        ));
        assert!(app.picker.editing_path);

        app.mode = Mode::Settings;
        app.settings = crate::app::Settings::default();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let settings_screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(settings_screen.contains("Auto-fetch remotes"));
        assert!(settings_screen.contains("Fetch interval"));
        assert!(!settings_screen.contains('┌'));
        assert!(app.regions.auto_fetch.is_some());
        assert!(app.regions.fetch_interval_up.is_some());

        app.mode = Mode::Help;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let help_screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(help_screen.contains("KEYBOARD"));
        assert!(help_screen.contains("Ctrl+Enter"));
        assert!(!help_screen.contains('┌'));

        let mut narrow = Terminal::new(TestBackend::new(50, 12)).unwrap();
        narrow.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn toggles_worktree_directories_with_the_mouse() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        run_git(root, &["init", "-b", "main"]);
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/app.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.worktree_rows().len(), 2);

        let worktree = app.regions.worktree_list.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            worktree.x + 1,
            worktree.y,
        ));
        let rows = app.worktree_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].directory_expanded, Some(false));

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let worktree = app.regions.worktree_list.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            worktree.x + 1,
            worktree.y,
        ));
        assert_eq!(app.worktree_rows().len(), 2);
    }

    #[test]
    fn styles_source_diff_with_numbers_and_tinted_changes() {
        let lines = styled_diff(
            "@@ -1 +1 @@\n-let old_value = 1;\n+let new_value = 2;",
            "src/main.rs",
            100,
        );

        assert_eq!(lines[0].style.bg, Some(palette().surface_alt));
        assert_eq!(lines[1].style.bg, Some(palette().remove_bg));
        assert_eq!(lines[2].style.bg, Some(palette().add_bg));
        assert!(lines[1].spans[0].content.contains('1'));
        assert!(lines[2].spans[0].content.contains('1'));
        assert!(
            lines[2]
                .spans
                .iter()
                .any(|span| span.content == "let" && span.style.fg == Some(palette().purple))
        );
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn run_git(root: &std::path::Path, args: &[&str]) {
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
