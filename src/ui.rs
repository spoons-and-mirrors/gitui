use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap,
    },
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Mode, PickerAction, Regions, View, WorktreeRow},
    git::{Change, Commit},
};

const INK: Color = Color::Rgb(220, 216, 207);
const MUTED: Color = Color::Rgb(126, 123, 117);
const FAINT: Color = Color::Rgb(72, 72, 69);
const PANEL: Color = Color::Rgb(41, 45, 62);
const SELECTED: Color = Color::Rgb(55, 60, 82);
const ACCENT: Color = Color::Rgb(151, 176, 225);
const GREEN: Color = Color::Rgb(145, 190, 145);
const YELLOW: Color = Color::Rgb(215, 185, 122);
const RED: Color = Color::Rgb(213, 130, 128);
const CYAN: Color = Color::Rgb(124, 186, 190);

const GRAPH_COLORS: [Color; 8] = [
    Color::Rgb(130, 170, 225),
    Color::Rgb(215, 150, 145),
    Color::Rgb(145, 190, 145),
    Color::Rgb(210, 180, 115),
    Color::Rgb(177, 145, 210),
    Color::Rgb(120, 190, 190),
    Color::Rgb(218, 155, 195),
    Color::Rgb(170, 170, 160),
];

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(PANEL).fg(INK)),
        frame.area(),
    );

    if frame.area().width < 60 || frame.area().height < 16 {
        frame.render_widget(
            Paragraph::new("Git Panel needs at least 60 columns and 16 rows\n\nq  quit")
                .alignment(Alignment::Center)
                .style(Style::default().fg(INK)),
            frame.area(),
        );
        return;
    }

    let layout = Layout::vertical([Constraint::Length(3), Constraint::Min(6)]).split(frame.area());

    draw_header(frame, app, layout[0]);
    match app.view {
        View::Changes => draw_changes(frame, app, layout[1]),
        View::Graph => draw_graph(frame, app, layout[1]),
    }
    match app.mode {
        Mode::Picker => draw_picker(frame, app),
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

    let mut title = vec![
        Span::styled(
            " GIT PANEL ",
            Style::default()
                .fg(PANEL)
                .bg(INK)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {path}"), Style::default().fg(INK)),
        Span::styled(format!("  {branch}"), Style::default().fg(ACCENT)),
    ];
    if let Some(notice) = &app.notice {
        title.push(Span::styled(
            format!("  {notice}"),
            Style::default().fg(YELLOW),
        ));
    }

    let changes_label = format!("[1 Changes {}/{}]", staged, unstaged);
    let graph_label = format!("[2 Graph {commits}]");
    let refresh_label = "[r Refresh]";
    let repository_label = "[o Repository]";
    let help_label = "[? Help]";
    let labels = [
        changes_label.as_str(),
        graph_label.as_str(),
        refresh_label,
        repository_label,
        help_label,
    ];

    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    let mut rects = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let active =
            (index == 0 && app.view == View::Changes) || (index == 1 && app.view == View::Graph);
        spans.push(Span::styled(
            *label,
            if active {
                Style::default()
                    .fg(PANEL)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            },
        ));
        rects.push(Rect::new(x, area.y + 1, label.len() as u16, 1));
        x = x.saturating_add(label.len() as u16 + 1);
        spans.push(Span::raw(" "));
    }

    app.regions = Regions {
        changes: rects.first().copied(),
        graph: rects.get(1).copied(),
        refresh: rects.get(2).copied(),
        repository: rects.get(3).copied(),
        help: rects.get(4).copied(),
        ..Regions::default()
    };

    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(title), Line::from(spans)])).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(FAINT)),
        ),
        area,
    );
}

fn draw_changes(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.repo.is_none() {
        draw_empty(frame, area, "Open a repository to inspect its worktree");
        return;
    }

    let left_width = area
        .width
        .saturating_mul(app.worktree_percent)
        .checked_div(100)
        .unwrap_or(0)
        .clamp(24, area.width.saturating_sub(24));
    let columns = [
        Rect::new(area.x, area.y, left_width, area.height),
        Rect::new(
            area.x.saturating_add(left_width),
            area.y,
            area.width.saturating_sub(left_width),
            area.height,
        ),
    ];
    app.regions.worktree = Some(columns[0]);
    app.regions.diff = Some(columns[1]);
    app.regions.split_bounds = Some(area);
    app.regions.splitter = Some(Rect::new(
        columns[0].right().saturating_sub(1),
        area.y,
        2,
        area.height,
    ));

    let worktree_content = columns[0].inner(Margin::new(1, 1));
    let commit_area = Rect::new(
        worktree_content.x,
        worktree_content.bottom().saturating_sub(4),
        worktree_content.width,
        4,
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
        GREEN
    } else if staged_count > 0 {
        YELLOW
    } else {
        MUTED
    };
    let worktree_header = Rect::new(
        worktree_content.x,
        worktree_content.y,
        worktree_content.width,
        1,
    );
    let worktree_list = Rect::new(
        worktree_header.x,
        worktree_header.y.saturating_add(1),
        worktree_header.width,
        commit_area
            .y
            .saturating_sub(worktree_header.y.saturating_add(1)),
    );
    app.regions.worktree_list = Some(worktree_list);
    app.regions.worktree_status = Some(Rect::new(
        worktree_list.right().saturating_sub(6),
        worktree_list.y,
        worktree_list.width.min(6),
        worktree_list.height,
    ));
    app.regions.stage_all = Some(Rect::new(
        worktree_header.right().saturating_sub(4),
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
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(SELECTED)
            .fg(INK)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(panel("WORKTREE", ""), columns[0]);
    let stage_label = format!(" Stage all  {} files", repo.changes.len());
    let stage_padding = usize::from(worktree_header.width)
        .saturating_sub(UnicodeWidthStr::width(stage_label.as_str()) + 4);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(stage_label, Style::default().fg(INK)),
            Span::raw(" ".repeat(stage_padding)),
            Span::styled(
                checkbox,
                Style::default()
                    .fg(checkbox_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ])),
        worktree_header,
    );
    frame.render_stateful_widget(list, worktree_list, &mut app.changes_state);

    let diff_lines = styled_diff(&app.diff);
    frame.render_widget(
        Paragraph::new(diff_lines)
            .block(panel("DIFF", "j/k select"))
            .scroll((app.diff_scroll, 0))
            .wrap(Wrap { trim: false }),
        columns[1],
    );

    let commit_active = app.mode == Mode::Commit;
    let commit_text = if app.commit_running {
        Line::styled("  Creating commit...", Style::default().fg(MUTED))
    } else if commit_active {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(&app.commit_message, Style::default().fg(INK)),
            Span::styled("█", Style::default().fg(ACCENT)),
        ])
    } else {
        Line::styled("  Write a commit message", Style::default().fg(MUTED))
    };
    frame.render_widget(
        Paragraph::new(commit_text)
            .style(Style::default().bg(PANEL))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(if commit_active { ACCENT } else { FAINT }))
                    .style(Style::default().bg(PANEL)),
            ),
        commit_area,
    );
}

fn draw_graph(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let Some(repo) = &app.repo else {
        draw_empty(frame, area, "Open a repository to inspect its graph");
        return;
    };
    app.regions.graph_table = Some(area);

    if repo.commits.is_empty() {
        draw_empty(frame, area, "This repository has no commits yet");
        return;
    }

    let maximum_graph_width = area.width.saturating_sub(35).clamp(8, 40);
    let graph_width = repo
        .commits
        .iter()
        .map(|commit| commit.graph.len())
        .max()
        .unwrap_or(1)
        .clamp(8, maximum_graph_width as usize) as u16;
    let compact = area.width < 110;
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
    .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    let table = Table::new(rows, widths)
        .header(headers)
        .column_spacing(1)
        .block(panel("ALL BRANCHES", "date order  j/k navigate"))
        .row_highlight_style(
            Style::default()
                .bg(SELECTED)
                .fg(INK)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(table, area, &mut app.graph_state);
}

fn graph_row(commit: &Commit, compact: bool) -> Row<'static> {
    let graph = Line::from(
        commit
            .graph
            .iter()
            .map(|cell| {
                Span::styled(
                    cell.symbol.to_string(),
                    Style::default().fg(GRAPH_COLORS[cell.color % GRAPH_COLORS.len()]),
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
        description.push(ref_badge("HEAD", GREEN));
        description.push(Span::raw(" "));
    }
    for reference in &commit.refs {
        let (label, color) = if let Some(tag) = reference.strip_prefix("tag: ") {
            (tag, YELLOW)
        } else if let Some(branch) = reference.strip_prefix("HEAD -> ") {
            (branch, ACCENT)
        } else if reference == "HEAD" {
            continue;
        } else {
            (reference.as_str(), ACCENT)
        };
        description.push(ref_badge(label, color));
        description.push(Span::raw(" "));
    }
    description.push(Span::styled(
        commit.subject.clone(),
        Style::default().fg(INK),
    ));

    let short_oid: String = commit.oid.chars().take(7).collect();
    if compact {
        Row::new([
            Cell::from(graph),
            Cell::from(Line::from(description)),
            Cell::from(commit.author.clone()).style(Style::default().fg(MUTED)),
            Cell::from(short_oid).style(Style::default().fg(MUTED)),
        ])
    } else {
        Row::new([
            Cell::from(graph),
            Cell::from(Line::from(description)),
            Cell::from(commit.date.clone()).style(Style::default().fg(MUTED)),
            Cell::from(commit.author.clone()).style(Style::default().fg(MUTED)),
            Cell::from(short_oid).style(Style::default().fg(MUTED)),
        ])
    }
}

fn ref_badge(label: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(PANEL)
            .bg(color)
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
                Style::default().fg(INK).add_modifier(Modifier::BOLD),
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
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(PANEL)),
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
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                &app.picker.path_input,
                Style::default().fg(if app.picker.editing_path { INK } else { MUTED }),
            ),
            Span::styled(
                if app.picker.editing_path { "▌" } else { "" },
                Style::default().fg(ACCENT),
            ),
        ])),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "LOCATION  ",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.picker.directory.display().to_string(),
                Style::default().fg(INK),
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
        let color = if entry.is_repo { GREEN } else { MUTED };
        ListItem::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(color)),
            Span::styled(entry.label.clone(), Style::default().fg(INK)),
        ]))
    });
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(
                Style::default()
                    .bg(SELECTED)
                    .fg(INK)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› "),
        parts[2],
        &mut app.picker.state,
    );
    if let Some(error) = &app.picker.error {
        frame.render_widget(
            Paragraph::new(error.as_str()).style(Style::default().fg(RED)),
            parts[3],
        );
    }
}

fn draw_help(frame: &mut Frame<'_>) {
    let area = centered(frame.area(), 62, 72);
    frame.render_widget(Clear, area);
    let help = vec![
        Line::styled(
            "NAVIGATION",
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ),
        help_line("1 / 2 / Tab", "Changes / Graph"),
        help_line("j / k", "Move selection"),
        help_line("g / G", "First / last row"),
        help_line("r", "Refresh repository"),
        help_line("o", "Open repository picker"),
        Line::raw(""),
        Line::styled(
            "WORKTREE",
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ),
        help_line("Space", "Stage or unstage selected file"),
        help_line("a / u", "Stage all / unstage all"),
        help_line("c", "Write commit message"),
        help_line("Esc", "Cancel current input or dialog"),
        help_line("q", "Quit"),
    ];
    frame.render_widget(
        Paragraph::new(help)
            .block(
                Block::default()
                    .title(Span::styled(
                        " KEYBOARD ",
                        Style::default().fg(INK).add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(Line::from(" ? / esc close ").alignment(Alignment::Right))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT))
                    .style(Style::default().bg(PANEL)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn worktree_item<'a>(row: &'a WorktreeRow, changes: &'a [Change], width: usize) -> ListItem<'a> {
    let Some(change_index) = row.change_index else {
        let directory = truncate_width(&format!("{}{}/", row.prefix, row.label), width);
        return ListItem::new(Line::from(Span::styled(
            directory,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
    };
    let change = &changes[change_index];
    let (section, color) = if change.staged {
        ("S", GREEN)
    } else {
        ("W", YELLOW)
    };
    let label = change.original_path.as_ref().map_or_else(
        || row.label.clone(),
        |original| {
            let original_name = original.rsplit('/').next().unwrap_or(original);
            format!("{original_name} → {}", row.label)
        },
    );
    let available_label = width.saturating_sub(6);
    let path = truncate_width(&format!("{}{}", row.prefix, label), available_label);
    let padding = available_label.saturating_sub(UnicodeWidthStr::width(path.as_str()));
    ListItem::new(Line::from(vec![
        Span::styled(path, Style::default().fg(INK)),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            format!(" {section} "),
            Style::default()
                .fg(PANEL)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} ", change.code), Style::default().fg(color)),
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

fn styled_diff(diff: &str) -> Vec<Line<'_>> {
    diff.lines()
        .map(|line| {
            let style = if line.starts_with("+++") || line.starts_with("---") {
                Style::default().fg(MUTED)
            } else if line.starts_with('+') {
                Style::default().fg(GREEN)
            } else if line.starts_with('-') {
                Style::default().fg(RED)
            } else if line.starts_with("@@") {
                Style::default().fg(CYAN)
            } else if line.starts_with("diff ") || line.starts_with("index ") {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(INK)
            };
            Line::styled(line, style)
        })
        .collect()
}

fn draw_empty(frame: &mut Frame<'_>, area: Rect, message: &str) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                message,
                Style::default().fg(INK).add_modifier(Modifier::BOLD),
            ),
            Line::styled("Press o to choose a directory", Style::default().fg(MUTED)),
        ])
        .alignment(Alignment::Center)
        .block(panel("", "")),
        area,
    );
}

fn panel<'a>(title: &'a str, hint: &'a str) -> Block<'a> {
    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(FAINT))
        .style(Style::default().bg(PANEL));
    if hint.is_empty() {
        block
    } else {
        block.title_bottom(Line::from(format!(" {hint} ")).alignment(Alignment::Right))
    }
}

fn help_line<'a>(key: &'a str, description: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!(" {key:<16}"),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(description, Style::default().fg(INK)),
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

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
        fs::write(root.join("tracked.txt"), "changed\n").unwrap();
        fs::write(root.join("untracked.txt"), "new\n").unwrap();

        let mut app = App::new(root.to_path_buf());
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
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
        let target = bounds.x + bounds.width * 65 / 100;
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
        assert!(app.worktree_percent >= 64);
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
        let commit = app.regions.commit.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            commit.x + 2,
            commit.y + 1,
        ));
        assert_eq!(app.mode, Mode::Commit);

        app.view = View::Graph;
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

        app.mode = Mode::Picker;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let path = app.regions.picker_path.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            path.x,
            path.y,
        ));
        assert!(app.picker.editing_path);

        let mut narrow = Terminal::new(TestBackend::new(50, 12)).unwrap();
        narrow.draw(|frame| draw(frame, &mut app)).unwrap();
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
