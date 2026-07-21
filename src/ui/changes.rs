use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, DiffHunkRegion, LeftPane, Mode, TextInput, View},
    git::{Change, DiffSummary},
    tree::{ExplorerRow, WorktreeRow, WorktreeSection},
};

use super::{
    fill, history, palette,
    preview::{PreparedPreview, PreviewInput},
    truncate_width,
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.repository().is_none() {
        super::draw_empty(frame, area, "Open a repository to inspect its changes");
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
    if app.changes.pane == LeftPane::Files {
        draw_explorer_changes(frame, app, columns);
        return;
    }

    let worktree_content = columns[0].inner(Margin::new(1, 0));
    let repo = app.session.data().expect("checked above");
    let local_workspace = repo.is_local();
    let staged_count = repo.change_counts.0;
    let checkbox = if !repo.changes.is_empty() && staged_count == repo.changes.len() {
        "◉"
    } else if staged_count > 0 {
        "◐"
    } else {
        "○"
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
    let commit_area = Rect::new(
        worktree_content.x,
        worktree_header.y.saturating_add(2),
        worktree_content.width,
        5,
    );
    app.regions.commit = Some(commit_area);
    let actions_row = Rect::new(
        worktree_content.x,
        commit_area.bottom(),
        worktree_content.width,
        1,
    );
    let worktree_list_y = actions_row.bottom();
    let maximum_history = worktree_content
        .bottom()
        .saturating_sub(worktree_list_y)
        .saturating_sub(2)
        .max(3);
    let history_height = app
        .settings
        .history_height
        .clamp(3, maximum_history)
        .min(worktree_content.bottom().saturating_sub(worktree_list_y));
    let history_area = Rect::new(
        worktree_content.x,
        worktree_content.bottom().saturating_sub(history_height),
        worktree_content.width,
        history_height,
    );
    let worktree_list = Rect::new(
        worktree_header.x,
        worktree_list_y,
        worktree_header.width,
        history_area.y.saturating_sub(worktree_list_y),
    );
    app.regions.worktree_list = Some(worktree_list);
    app.regions.worktree_status = Some(Rect::new(
        worktree_list.right().saturating_sub(2),
        worktree_list.y,
        worktree_list.width.min(2),
        worktree_list.height,
    ));
    app.regions.stage_all = Some(Rect::new(
        worktree_header.right().saturating_sub(2),
        worktree_header.y,
        worktree_header.width.min(2),
        1,
    ));
    app.regions.unstage_all = None;
    app.regions.history_bounds = Some(Rect::new(
        worktree_content.x,
        worktree_list_y.saturating_add(2),
        worktree_content.width,
        worktree_content
            .bottom()
            .saturating_sub(worktree_list_y.saturating_add(2)),
    ));
    app.regions.history_splitter = Some(Rect::new(
        history_area.x,
        history_area.y,
        history_area.width,
        1,
    ));
    app.regions.history_list = Some(Rect::new(
        history_area.x,
        history_area.y.saturating_add(1),
        history_area.width,
        history_area.height.saturating_sub(1),
    ));

    let worktree_len = app.changes.worktree_rows(repo).len();
    let worktree_viewport = usize::from(worktree_list.height);
    app.changes.worktree_scroll = app
        .changes
        .worktree_scroll
        .min(worktree_len.saturating_sub(worktree_viewport));
    if app.changes.worktree_scroll_to_selection
        && worktree_viewport > 0
        && let Some(selected) = app.changes.worktree_state.selected()
    {
        if selected < app.changes.worktree_scroll {
            app.changes.worktree_scroll = selected;
        } else if selected
            >= app
                .changes
                .worktree_scroll
                .saturating_add(worktree_viewport)
        {
            app.changes.worktree_scroll =
                selected.saturating_add(1).saturating_sub(worktree_viewport);
        }
    }
    app.changes.worktree_scroll_to_selection = false;
    let selected_style = Style::default().bg(if app.mode == Mode::Commit {
        palette().inactive_selected
    } else {
        palette().selected
    });
    let items: Vec<ListItem<'_>> = app
        .changes
        .worktree_rows(repo)
        .iter()
        .enumerate()
        .skip(app.changes.worktree_scroll)
        .take(worktree_viewport)
        .map(|(index, row)| {
            let item = worktree_item(row, &repo.changes, worktree_list.width as usize);
            if app.changes.worktree_state.selected() == Some(index) {
                item.style(selected_style)
            } else {
                item
            }
        })
        .collect();
    let list = List::new(items);
    let stage_label = if worktree_header.width >= 36 {
        format!("Stage all  {} files", repo.changes.len())
    } else {
        "All".to_owned()
    };
    let worktree_title = if worktree_header.width >= 36 {
        format!("CHANGES  {}", repo.changes.len())
    } else {
        "CHANGES".to_owned()
    };
    let files_title = "FILES";
    let worktree_title_width = UnicodeWidthStr::width(worktree_title.as_str());
    let title_width = worktree_title_width + 2 + files_title.len();
    let stage_width = UnicodeWidthStr::width(stage_label.as_str()) + 3;
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
            Span::raw("  "),
            Span::styled(files_title, Style::default().fg(palette().faint)),
            Span::raw(" ".repeat(stage_padding)),
            Span::styled(
                format!("{stage_label} "),
                Style::default().fg(palette().muted),
            ),
            Span::styled(
                format!("{checkbox} "),
                Style::default()
                    .fg(checkbox_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        worktree_header,
    );
    app.regions.worktree_tab = Some(Rect::new(
        worktree_header.x,
        worktree_header.y,
        worktree_title_width as u16,
        1,
    ));
    app.regions.files_tab = Some(Rect::new(
        worktree_header
            .x
            .saturating_add(worktree_title_width as u16 + 2),
        worktree_header.y,
        files_title.len() as u16,
        1,
    ));
    frame.render_widget(list, worktree_list);

    let history_header = app.regions.history_splitter.expect("set above");
    let history_list = app.regions.history_list.expect("set above");
    app.regions.actions = if local_workspace {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "LOCAL WORKSPACE",
                Style::default().fg(palette().faint),
            )),
            actions_row,
        );
        None
    } else {
        Some(draw_actions(frame, actions_row, app.mode))
    };
    history::draw_branch(
        frame,
        &repo.history,
        &repo.branch,
        history_header,
        history_list,
        app.dragging_history,
        app.changes.history_focused,
        app.mode,
        &mut app.changes.history_state,
    );

    let selected_history = if app.changes.history_focused {
        app.changes
            .history_state
            .selected()
            .and_then(|index| repo.history.get(index))
    } else {
        None
    };
    let selected_graph_commit = (app.view == View::Graph && app.graph_commit_open)
        .then(|| app.selected_graph_commit())
        .flatten();
    let selected_commit = selected_history.or(selected_graph_commit);
    let selected_change = if selected_commit.is_none() {
        app.changes
            .worktree_state
            .selected()
            .and_then(|index| app.changes.worktree_rows(repo).get(index))
            .and_then(|row| row.change_index)
            .and_then(|index| repo.changes.get(index))
    } else {
        None
    };
    let selected_label = selected_commit.map_or_else(
        || {
            selected_change
                .map_or("No file selected", |change| change.path.as_str())
                .to_owned()
        },
        |commit| commit.subject.clone(),
    );
    let syntax_path = selected_change.map_or_else(String::new, |change| change.path.clone());
    let diff_header = Rect::new(
        columns[1].x.saturating_add(1),
        columns[1].y.saturating_add(1),
        columns[1].width.saturating_sub(2),
        1,
    );
    let state = selected_commit.map_or_else(
        || {
            selected_change.map_or(
                "",
                |change| {
                    if change.staged { "staged" } else { "unstaged" }
                },
            )
        },
        |_| "commit",
    );
    let inspecting_commit = selected_commit.is_some();
    let show_summary = inspecting_commit || selected_change.is_some();
    let live_summary = selected_change.map(|change| DiffSummary {
        files: vec![change.path.clone()],
        additions: change.additions,
        deletions: change.deletions,
    });
    let summary = selected_commit
        .and_then(|commit| app.commit_summaries.get(&commit.oid))
        .or(live_summary.as_ref());
    let summary_unavailable =
        selected_commit.is_some_and(|commit| app.commit_summaries.failed(&commit.oid));
    let maximum_summary_height = columns[1]
        .height
        .saturating_sub(7)
        .clamp(3, 8)
        .min(columns[1].height);
    let summary_height = if show_summary {
        diff_summary_height(
            summary,
            diff_header.width,
            app.changes.diff_wrap,
            maximum_summary_height,
        )
    } else {
        0
    };
    let diff_body = Rect::new(
        diff_header.x,
        diff_header
            .y
            .saturating_add(2)
            .saturating_add(summary_height),
        diff_header.width,
        columns[1].bottom().saturating_sub(
            diff_header
                .y
                .saturating_add(3)
                .saturating_add(summary_height),
        ),
    );
    let wrap_label = if app.changes.diff_wrap {
        "  alt+w:on"
    } else {
        "  alt+w:off"
    };
    let display_path = truncate_width(
        &selected_label,
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
                Style::default().fg(match state {
                    "staged" => palette().green,
                    "commit" => palette().accent,
                    _ => palette().yellow,
                }),
            ),
            Span::styled(
                wrap_label,
                Style::default().fg(if app.changes.diff_wrap {
                    palette().accent
                } else {
                    palette().faint
                }),
            ),
        ])),
        diff_header,
    );
    if show_summary {
        draw_diff_summary(
            frame,
            Rect::new(
                diff_header.x,
                diff_header.y.saturating_add(2),
                diff_header.width,
                summary_height.saturating_sub(1),
            ),
            summary,
            summary_unavailable,
            app.changes.diff_wrap,
        );
    }
    let show_hunk_actions =
        !inspecting_commit && selected_change.is_some_and(|change| !change.staged);
    let mut preview = prepare_preview_lines(app, diff_body, &syntax_path, true, inspecting_commit);
    let (hunk_rows, rendered_height) = if show_hunk_actions {
        app.changes
            .preview_presentation
            .hunk_rows(&app.changes.diff, preview.wrapped)
    } else {
        (Vec::new(), 0)
    };
    let pin_hunk = app.changes.take_hunk_pin_request();
    if pin_hunk
        && let Some(selected) = app.changes.hunk_selection
        && let Some((_, row)) = hunk_rows.iter().find(|(index, _)| *index == selected)
    {
        let old_scroll = app.changes.diff_scroll;
        app.changes.diff_scroll = scroll_to_row(*row, rendered_height);
        if app.changes.diff_scroll != old_scroll {
            preview = prepare_preview_lines(app, diff_body, &syntax_path, true, inspecting_commit);
        }
    }
    let visible_hunks = visible_hunks(
        &hunk_rows,
        rendered_height,
        diff_body,
        app.changes.diff_scroll,
    );
    render_scrollable_content(frame, app, columns[1], diff_body, preview);
    draw_hunk_actions(frame, app, diff_body, visible_hunks);

    let commit_active = app.mode == Mode::Commit;
    fill(frame, commit_area, palette().canvas);
    let commit_content = commit_area.inner(Margin::new(1, 0));
    let (commit_text, commit_height) = if local_workspace {
        (
            Text::from(vec![
                Line::styled(
                    "Local file workspace",
                    Style::default()
                        .fg(palette().muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "Git status and commits are unavailable",
                    Style::default().fg(palette().faint),
                ),
            ]),
            2,
        )
    } else if app.commit_running() {
        (
            Text::from(Line::styled(
                "Creating commit...",
                Style::default().fg(palette().yellow),
            )),
            1,
        )
    } else if commit_active || !app.commit_input.is_empty() {
        let lines = commit_input_lines(&app.commit_input, commit_active);
        let height = rendered_text_height(&lines, usize::from(commit_content.width), true);
        (Text::from(lines), height)
    } else {
        let hint = "Ctrl+Enter commit";
        let placeholder = "Write a commit message";
        if commit_content.width >= 40 {
            let padding =
                usize::from(commit_content.width).saturating_sub(placeholder.len() + hint.len());
            (
                Text::from(Line::from(vec![
                    Span::styled(placeholder, Style::default().fg(palette().muted)),
                    Span::raw(" ".repeat(padding)),
                    Span::styled(hint, Style::default().fg(palette().faint)),
                ])),
                1,
            )
        } else {
            (
                Text::from(Line::styled(
                    placeholder,
                    Style::default().fg(palette().muted),
                )),
                1,
            )
        }
    };
    let commit_scroll = if commit_active {
        commit_cursor_row(&app.commit_input, usize::from(commit_content.width))
            .saturating_sub(usize::from(commit_content.height).saturating_sub(1))
    } else {
        commit_height.saturating_sub(usize::from(commit_content.height))
    }
    .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(
        Paragraph::new(commit_text)
            .wrap(Wrap { trim: false })
            .scroll((commit_scroll, 0))
            .style(Style::default().bg(palette().canvas)),
        commit_content,
    );
}

fn commit_input_lines(input: &TextInput, active: bool) -> Vec<Line<'static>> {
    let selection = active.then(|| input.selection()).flatten();
    let mut line_start = 0;
    input
        .text()
        .split('\n')
        .map(|line| {
            if !active {
                line_start += line.len() + 1;
                return Line::styled(line.to_owned(), Style::default().fg(palette().muted));
            }

            let mut spans = Vec::new();
            for (offset, character) in line.char_indices() {
                let index = line_start + offset;
                let selected = selection.is_some_and(|(start, end)| start <= index && index < end);
                let cursor = input.cursor_visible() && input.cursor() == index;
                let style = if cursor {
                    Style::default().fg(palette().canvas).bg(palette().accent)
                } else if selected {
                    Style::default().fg(palette().ink).bg(palette().selected)
                } else {
                    Style::default().fg(palette().ink)
                };
                spans.push(Span::styled(character.to_string(), style));
            }
            if input.cursor() == line_start + line.len() {
                spans.push(Span::styled(
                    " ",
                    if input.cursor_visible() {
                        Style::default().bg(palette().accent)
                    } else {
                        Style::default()
                    },
                ));
            }
            line_start += line.len() + 1;
            Line::from(spans)
        })
        .collect()
}

fn commit_cursor_row(input: &TextInput, width: usize) -> usize {
    let width = width.max(1);
    let mut row = 0;
    let mut lines = input.text()[..input.cursor()].split('\n').peekable();
    while let Some(line) = lines.next() {
        let line_width = UnicodeWidthStr::width(line);
        if lines.peek().is_some() {
            row += line_width.saturating_sub(1) / width + 1;
        } else {
            row += line_width / width;
        }
    }
    row
}

fn draw_actions(frame: &mut Frame<'_>, area: Rect, mode: Mode) -> Rect {
    let label = " x ACTIONS ▾ ";
    let width = (UnicodeWidthStr::width(label) as u16).min(area.width);
    let button = Rect::new(area.right().saturating_sub(width), area.y, width, 1);
    fill(frame, area, palette().panel);
    frame.render_widget(
        Paragraph::new(Line::styled(
            label,
            Style::default()
                .fg(palette().accent)
                .bg(if mode == Mode::ActionMenu {
                    palette().selected
                } else {
                    palette().raised
                })
                .add_modifier(Modifier::BOLD),
        )),
        button,
    );
    button
}

fn draw_explorer_changes(frame: &mut Frame<'_>, app: &mut App, columns: [Rect; 2]) {
    app.regions.worktree_list = None;
    app.regions.worktree_status = None;
    app.regions.stage_all = None;
    app.regions.commit = None;
    app.regions.history_list = None;
    app.regions.history_splitter = None;
    app.regions.history_bounds = None;

    let content = columns[0].inner(Margin::new(1, 0));
    let header = Rect::new(content.x, content.y.saturating_add(1), content.width, 1);
    let list_area = Rect::new(
        content.x,
        header.y.saturating_add(2),
        content.width,
        content.bottom().saturating_sub(header.y.saturating_add(2)),
    );
    let file_count = app.repository().map_or(0, |repo| repo.files.len());
    let files_title = if header.width >= 30 {
        format!("FILES  {file_count}")
    } else {
        "FILES".to_owned()
    };
    let add_width = 5.min(header.width);
    let add_button = Rect::new(
        header.right().saturating_sub(add_width),
        header.y,
        add_width,
        1,
    );
    let root_target = Rect::new(
        header.x,
        header.y,
        header.width.saturating_sub(add_width),
        1,
    );
    let drop_target = app.file_drop_target().map(str::to_owned);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("CHANGES", Style::default().fg(palette().faint)),
            Span::raw("  "),
            Span::styled(
                files_title.clone(),
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        root_target,
    );
    frame.render_widget(
        Paragraph::new(" + ")
            .alignment(Alignment::Center)
            .style(Style::default().fg(palette().accent).bg(palette().raised)),
        add_button,
    );
    if drop_target.as_deref() == Some("") {
        frame.render_widget(
            Block::default().style(Style::default().bg(palette().inactive_selected)),
            root_target,
        );
        frame.render_widget(
            Paragraph::new(format!("CHANGES  {files_title}"))
                .style(Style::default().fg(palette().ink)),
            root_target,
        );
    }
    app.regions.worktree_tab = Some(Rect::new(header.x, header.y, 7, 1));
    app.regions.files_tab = Some(Rect::new(
        header.x.saturating_add(9),
        header.y,
        UnicodeWidthStr::width(files_title.as_str()) as u16,
        1,
    ));
    app.regions.explorer_list = Some(list_area);
    app.regions.files_add = Some(add_button);
    app.regions.files_root = Some(root_target);

    let viewport = usize::from(list_area.height);
    let row_count = app.changes.explorer_rows().len();
    app.changes.explorer_scroll = app
        .changes
        .explorer_scroll
        .min(row_count.saturating_sub(viewport));
    let rows = app.changes.explorer_rows();
    let items: Vec<ListItem<'_>> = if rows.is_empty() {
        vec![ListItem::new(Line::styled(
            " No files",
            Style::default().fg(palette().faint),
        ))]
    } else {
        rows.iter()
            .enumerate()
            .skip(app.changes.explorer_scroll)
            .take(viewport)
            .map(|(index, row)| {
                let repo = app.repository().expect("checked above");
                let path = row
                    .file_index
                    .and_then(|file_index| repo.files.get(file_index))
                    .map(String::as_str)
                    .or(row.directory_path.as_deref());
                let code = path.and_then(|path| app.changes.explorer_change_code(path));
                let item = explorer_item(row, code, usize::from(list_area.width));
                if app.changes.explorer_state.selected() == Some(index) {
                    item.style(Style::default().bg(palette().selected))
                } else if drop_target.as_deref().is_some_and(|target| {
                    row.directory_path
                        .as_deref()
                        .is_some_and(|path| path == target)
                }) {
                    item.style(Style::default().bg(palette().inactive_selected))
                } else {
                    item
                }
            })
            .collect()
    };
    frame.render_widget(List::new(items), list_area);

    let selected_path = app
        .selected_explorer_file_path()
        .unwrap_or("No file selected")
        .to_owned();
    let preview_header = Rect::new(
        columns[1].x.saturating_add(1),
        columns[1].y.saturating_add(1),
        columns[1].width.saturating_sub(2),
        1,
    );
    let preview_body = Rect::new(
        preview_header.x,
        preview_header.y.saturating_add(2),
        preview_header.width,
        columns[1]
            .bottom()
            .saturating_sub(preview_header.y.saturating_add(3)),
    );
    let wrap_label = if app.changes.diff_wrap {
        "  alt+w:on"
    } else {
        "  alt+w:off"
    };
    let display_path = truncate_width(
        &selected_path,
        usize::from(preview_header.width)
            .saturating_sub(7 + "read-only".len() + UnicodeWidthStr::width(wrap_label)),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "FILE  ",
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
            Span::styled("  read-only", Style::default().fg(palette().accent)),
            Span::styled(
                wrap_label,
                Style::default().fg(if app.changes.diff_wrap {
                    palette().accent
                } else {
                    palette().faint
                }),
            ),
        ])),
        preview_header,
    );
    let path = app
        .selected_explorer_file_path()
        .unwrap_or_default()
        .to_owned();
    let preview = prepare_preview_lines(app, preview_body, &path, false, false);
    render_scrollable_content(frame, app, columns[1], preview_body, preview);
}

fn prepare_preview_lines(
    app: &mut App,
    body: Rect,
    path: &str,
    is_diff: bool,
    show_initial_diff_header: bool,
) -> PreparedPreview {
    app.changes.preview_presentation.prepare(
        PreviewInput {
            content: &app.changes.diff,
            generation: app.changes.preview_content_generation,
            path,
            is_diff,
            show_initial_diff_header,
            width: usize::from(body.width),
            viewport_height: usize::from(body.height),
            wrapped: app.changes.diff_wrap,
            hunk_selected: app.changes.hunk_selection.is_some(),
        },
        &mut app.changes.diff_scroll,
    )
}

fn render_scrollable_content(
    frame: &mut Frame<'_>,
    app: &mut App,
    panel: Rect,
    body: Rect,
    preview: PreparedPreview,
) {
    let rendered_height = preview.rendered_height;
    let paragraph = Paragraph::new(preview.lines).style(Style::default().bg(palette().panel));
    let viewport_height = usize::from(body.height);
    let max_scroll = rendered_height.saturating_sub(viewport_height);
    let scroll_limit = if app.changes.hunk_selection.is_some() {
        rendered_height.saturating_sub(1)
    } else {
        max_scroll
    };
    app.regions.diff_scroll_max = max_scroll;
    app.changes.diff_scroll = app.changes.diff_scroll.min(scroll_limit);
    let scrollbar = Rect::new(panel.right().saturating_sub(1), body.y, 1, body.height);
    app.regions.diff_scrollbar = Some(scrollbar);
    app.regions.diff_scroll_thumb = (max_scroll > 0).then(|| {
        diff_scroll_thumb(
            scrollbar,
            rendered_height,
            viewport_height,
            app.changes.diff_scroll.min(max_scroll),
            max_scroll,
        )
    });
    frame.render_widget(paragraph, body);
    if let Some(thumb) = app.regions.diff_scroll_thumb {
        frame.render_widget(
            Paragraph::new(Text::from(
                (0..scrollbar.height)
                    .map(|_| Line::styled("│", Style::default().fg(palette().faint)))
                    .collect::<Vec<_>>(),
            )),
            scrollbar,
        );
        frame.render_widget(
            Paragraph::new(Text::from(
                (0..thumb.height)
                    .map(|_| {
                        Line::styled(
                            "┃",
                            Style::default().fg(if app.dragging_diff_scrollbar {
                                palette().accent
                            } else {
                                palette().muted
                            }),
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
            thumb,
        );
    }
}

fn draw_diff_summary(
    frame: &mut Frame<'_>,
    area: Rect,
    summary: Option<&DiffSummary>,
    unavailable: bool,
    wrapped: bool,
) {
    let stats_area = Rect::new(area.x, area.y, area.width, 1);
    let files_area = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let Some(summary) = summary else {
        let state = if unavailable {
            "unavailable"
        } else {
            "loading…"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "CHANGES  ",
                    Style::default()
                        .fg(palette().muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(state, Style::default().fg(palette().faint)),
            ])),
            stats_area,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("FILES  {state}"),
                Style::default().fg(palette().faint),
            )),
            files_area,
        );
        return;
    };

    let file_count = summary.files.len();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "CHANGES  ",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("+{}", summary.additions),
                Style::default().fg(palette().green),
            ),
            Span::raw("  "),
            Span::styled(
                format!("-{}", summary.deletions),
                Style::default().fg(palette().red),
            ),
            Span::styled(
                format!(
                    "  {file_count} {}",
                    if file_count == 1 { "file" } else { "files" }
                ),
                Style::default().fg(palette().faint),
            ),
        ])),
        stats_area,
    );
    let label = "FILES  ";
    let available = usize::from(area.width).saturating_sub(label.len());
    let file_lines = if wrapped {
        wrapped_file_summary(
            &summary.files.join("  "),
            available,
            usize::from(files_area.height),
        )
    } else {
        vec![truncate_width(&summary.files.join("  "), available)]
    };
    let lines = file_lines
        .into_iter()
        .enumerate()
        .map(|(index, files)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 { label } else { "       " },
                    Style::default()
                        .fg(palette().muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(files, Style::default().fg(palette().cyan)),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), files_area);
}

fn diff_summary_height(
    summary: Option<&DiffSummary>,
    width: u16,
    wrapped: bool,
    maximum: u16,
) -> u16 {
    if !wrapped {
        return 3.min(maximum);
    }
    let file_width = usize::from(width).saturating_sub("FILES  ".len());
    let rows = summary.map_or(1, |summary| {
        wrapped_file_summary(&summary.files.join("  "), file_width, usize::MAX)
            .len()
            .max(1)
    });
    (rows as u16).saturating_add(2).min(maximum)
}

fn wrapped_file_summary(text: &str, width: usize, maximum_lines: usize) -> Vec<String> {
    if width == 0 || maximum_lines == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0usize;
    let mut truncated = false;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if line_width > 0 && line_width.saturating_add(character_width) > width {
            if lines.len().saturating_add(1) >= maximum_lines {
                truncated = true;
                break;
            }
            lines.push(std::mem::take(&mut line));
            line_width = 0;
        }
        line.push(character);
        line_width = line_width.saturating_add(character_width);
    }
    if !line.is_empty() && lines.len() < maximum_lines {
        lines.push(line);
    }
    if truncated && let Some(last) = lines.last_mut() {
        *last = format!("{}…", truncate_width(last, width.saturating_sub(1)));
    }
    lines
}

struct VisibleHunk {
    index: usize,
    area: Rect,
    header_y: Option<u16>,
    continues_above: bool,
    continues_below: bool,
    scroll_start: usize,
    scroll_end: usize,
}

fn visible_hunks(
    rows: &[(usize, usize)],
    rendered_height: usize,
    body: Rect,
    scroll: usize,
) -> Vec<VisibleHunk> {
    let top = scroll;
    let bottom = top.saturating_add(usize::from(body.height));
    rows.iter()
        .enumerate()
        .filter_map(|(position, (index, header))| {
            let end = rows
                .get(position + 1)
                .map_or(rendered_height, |(_, next)| next.saturating_sub(1));
            let visible_start = (*header).max(top);
            let visible_end = end.min(bottom);
            let scroll_start = *header;
            let scroll_end = end.saturating_sub(usize::from(body.height)).max(*header);
            (visible_start < visible_end).then(|| VisibleHunk {
                index: *index,
                area: Rect::new(
                    body.x,
                    body.y.saturating_add((visible_start - top) as u16),
                    body.width,
                    (visible_end - visible_start) as u16,
                ),
                header_y: (*header >= top && *header < bottom)
                    .then(|| body.y.saturating_add((*header - top) as u16)),
                continues_above: *header < top,
                continues_below: end > bottom,
                scroll_start,
                scroll_end,
            })
        })
        .collect()
}

fn scroll_to_row(row: usize, rendered_height: usize) -> usize {
    row.min(rendered_height.saturating_sub(1))
}

fn draw_hunk_actions(frame: &mut Frame<'_>, app: &mut App, body: Rect, hunks: Vec<VisibleHunk>) {
    if body.width < 3 {
        return;
    }
    for hunk in hunks {
        let selected = app.changes.hunk_selection == Some(hunk.index);
        if selected && let Some(y) = hunk.header_y {
            frame.buffer_mut().set_style(
                Rect::new(body.x, y, body.width, 1),
                Style::default().bg(palette().selected),
            );
        }
        let action = hunk.header_y.map(|y| {
            let rect = Rect::new(body.right().saturating_sub(3), y, 3, 1);
            frame.render_widget(
                Paragraph::new("[+]").style(
                    Style::default()
                        .fg(if selected {
                            palette().ink
                        } else {
                            palette().green
                        })
                        .bg(if selected {
                            palette().accent
                        } else {
                            palette().raised
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                rect,
            );
            rect
        });
        app.regions.diff_hunks.push(DiffHunkRegion {
            rect: hunk.area,
            action,
            index: hunk.index,
            continues_above: hunk.continues_above,
            continues_below: hunk.continues_below,
            scroll_start: hunk.scroll_start,
            scroll_end: hunk.scroll_end,
        });
    }
}

fn worktree_item<'a>(row: &'a WorktreeRow, changes: &'a [Change], width: usize) -> ListItem<'a> {
    if let Some(section) = row.section {
        let Some((additions, deletions)) = row.section_stats else {
            return ListItem::new("");
        };
        let color = match section {
            WorktreeSection::Staged => palette().green,
            WorktreeSection::Unstaged => palette().yellow,
        };
        let additions = format!("+{additions}");
        let deletions = format!("-{deletions}");
        let stats_width = additions.len() + 1 + deletions.len();
        let show_stats = width >= stats_width + 4;
        let available_label = width.saturating_sub(usize::from(show_stats) * stats_width);
        let label = truncate_width(&format!(" {}", row.label), available_label);
        let padding = available_label.saturating_sub(UnicodeWidthStr::width(label.as_str()));
        let mut spans = vec![
            Span::styled(
                label,
                Style::default()
                    .fg(color)
                    .bg(palette().surface_alt)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ".repeat(padding),
                Style::default().bg(palette().surface_alt),
            ),
        ];
        if show_stats {
            spans.extend([
                Span::styled(
                    additions,
                    Style::default()
                        .fg(palette().green)
                        .bg(palette().surface_alt),
                ),
                Span::styled(" ", Style::default().bg(palette().surface_alt)),
                Span::styled(
                    deletions,
                    Style::default().fg(palette().red).bg(palette().surface_alt),
                ),
            ]);
        }
        return ListItem::new(Line::from(spans));
    }
    let Some(change_index) = row.change_index else {
        let marker = if row.directory_expanded == Some(false) {
            "▢ "
        } else {
            "▣ "
        };
        let directory = truncate_width(&format!("{}{}{}", row.prefix, marker, row.label), width);
        return ListItem::new(Line::from(Span::styled(directory, folder_style())));
    };
    let change = &changes[change_index];
    let (checkbox, color) = if change.staged {
        ("◉", palette().green)
    } else {
        ("○", palette().muted)
    };
    let label = change.original_path.as_ref().map_or_else(
        || row.label.clone(),
        |original| {
            let original_name = original.rsplit('/').next().unwrap_or(original);
            format!("{original_name} → {}", row.label)
        },
    );
    let additions = format!("+{}", change.additions);
    let deletions = format!("-{}", change.deletions);
    let stats_width = additions.len() + 1 + deletions.len();
    let show_stats = width >= stats_width + 10;
    let controls_width = 2 + usize::from(show_stats) * (stats_width + 1);
    let available_label = width.saturating_sub(controls_width);
    let path = truncate_width(&format!("{}{}", row.prefix, label), available_label);
    let padding = available_label.saturating_sub(UnicodeWidthStr::width(path.as_str()));
    let mut spans = vec![
        Span::styled(path, Style::default().fg(palette().ink)),
        Span::raw(" ".repeat(padding)),
    ];
    if show_stats {
        spans.extend([
            Span::styled(additions, Style::default().fg(palette().green)),
            Span::raw(" "),
            Span::styled(deletions, Style::default().fg(palette().red)),
            Span::raw(" "),
        ]);
    }
    spans.push(Span::styled(
        format!("{checkbox} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    ListItem::new(Line::from(spans))
}

fn explorer_item(row: &ExplorerRow, change_code: Option<char>, width: usize) -> ListItem<'static> {
    if row.file_index.is_none() {
        let marker = if row.directory_expanded == Some(false) {
            "▢ "
        } else {
            "▣ "
        };
        return ListItem::new(Line::styled(
            truncate_width(&format!("{}{}{}", row.prefix, marker, row.label), width),
            explorer_folder_style(change_code),
        ));
    }
    let prefix = truncate_width(&row.prefix, width);
    let label_width = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    let label = truncate_width(&row.label, label_width);
    let color = change_code
        .map(explorer_file_color)
        .unwrap_or(palette().ink);
    ListItem::new(Line::from(vec![
        Span::styled(prefix, Style::default().fg(palette().ink)),
        Span::styled(label, Style::default().fg(color)),
    ]))
}

fn explorer_file_color(code: char) -> ratatui::style::Color {
    match code {
        'D' | 'U' => palette().red,
        '?' => palette().green,
        'A' => palette().accent,
        'R' => palette().purple,
        'C' => palette().cyan,
        'M' => palette().yellow,
        _ => palette().orange,
    }
}

fn explorer_folder_style(change_code: Option<char>) -> Style {
    Style::default().fg(change_code
        .map(explorer_file_color)
        .unwrap_or(palette().muted))
}

fn folder_style() -> Style {
    Style::default().fg(palette().muted)
}

fn rendered_text_height(lines: &[Line<'_>], width: usize, wrapped: bool) -> usize {
    if !wrapped {
        return lines.len();
    }
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

fn diff_scroll_thumb(
    track: Rect,
    content_height: usize,
    viewport_height: usize,
    scroll: usize,
    max_scroll: usize,
) -> Rect {
    let thumb_height = (usize::from(track.height) * viewport_height)
        .checked_div(content_height.max(1))
        .unwrap_or(0)
        .max(1)
        .min(usize::from(track.height)) as u16;
    let travel = track.height.saturating_sub(thumb_height);
    let offset = ((scroll as u128 * u128::from(travel) + max_scroll as u128 / 2)
        .checked_div(max_scroll as u128)
        .unwrap_or(0)) as u16;
    Rect::new(
        track.x,
        track.y.saturating_add(offset),
        track.width,
        thumb_height,
    )
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn wraps_file_summaries_with_a_bounded_height() {
        let text = "src/one.rs  src/two.rs  src/three.rs  src/four.rs";
        let lines = wrapped_file_summary(text, 12, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.last().unwrap().ends_with('…'));
        assert!(
            lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 12)
        );

        let summary = DiffSummary {
            files: vec![text.to_owned()],
            additions: 1,
            deletions: 1,
        };
        assert_eq!(diff_summary_height(Some(&summary), 19, false, 8), 3);
        assert!(diff_summary_height(Some(&summary), 19, true, 8) > 3);
    }
}
