use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{List, ListItem, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, LeftPane, Mode},
    git::Change,
    tree::{ExplorerRow, WorktreeRow},
};

use super::{
    fill, history, palette,
    text::{styled_diff, styled_source},
    truncate_width,
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.repository().is_none() {
        super::draw_empty(frame, area, "Open a repository to inspect its worktree");
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
    let staged_count = repo.changes.iter().filter(|change| change.staged).count();
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
    let worktree_list_y = commit_area.bottom();
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

    let worktree_rows = app.changes.worktree_rows(repo);
    let worktree_viewport = usize::from(worktree_list.height);
    app.changes.worktree_scroll = app
        .changes
        .worktree_scroll
        .min(worktree_rows.len().saturating_sub(worktree_viewport));
    let selected_style = Style::default().bg(if app.mode == Mode::Commit {
        palette().inactive_selected
    } else {
        palette().selected
    });
    let items: Vec<ListItem<'_>> = worktree_rows
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
        format!("WORKTREE  {}", repo.changes.len())
    } else {
        "WORKTREE".to_owned()
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
    let selected_change = if selected_history.is_none() {
        app.changes
            .worktree_state
            .selected()
            .and_then(|index| worktree_rows.get(index))
            .and_then(|row| row.change_index)
            .and_then(|index| repo.changes.get(index))
    } else {
        None
    };
    let selected_label = selected_history.map_or_else(
        || {
            selected_change
                .map_or("No file selected", |change| change.path.as_str())
                .to_owned()
        },
        |commit| commit.subject.clone(),
    );
    let syntax_path = selected_change.map_or("", |change| change.path.as_str());
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
    let state = selected_history.map_or_else(
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
    let wrap_label = if app.changes.diff_wrap {
        "  w:on"
    } else {
        "  w:off"
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
    let diff_lines = styled_diff(&app.changes.diff, syntax_path, usize::from(diff_body.width));
    render_scrollable_content(frame, app, columns[1], diff_body, diff_lines);

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
    let (commit_text, commit_height) = if app.commit_running() {
        (
            Text::from(Line::styled(
                "Creating commit...",
                Style::default().fg(palette().yellow),
            )),
            1,
        )
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
    let commit_scroll = commit_height
        .saturating_sub(usize::from(commit_content.height))
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(
        Paragraph::new(commit_text)
            .wrap(Wrap { trim: false })
            .scroll((commit_scroll, 0))
            .style(Style::default().bg(palette().canvas)),
        commit_content,
    );
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("WORKTREE", Style::default().fg(palette().faint)),
            Span::raw("  "),
            Span::styled(
                files_title.clone(),
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        header,
    );
    app.regions.worktree_tab = Some(Rect::new(header.x, header.y, 8, 1));
    app.regions.files_tab = Some(Rect::new(
        header.x.saturating_add(10),
        header.y,
        UnicodeWidthStr::width(files_title.as_str()) as u16,
        1,
    ));
    app.regions.explorer_list = Some(list_area);

    let viewport = usize::from(list_area.height);
    let row_count = app.changes.explorer_rows().len();
    app.changes.explorer_scroll = app
        .changes
        .explorer_scroll
        .min(row_count.saturating_sub(viewport));
    let rows = app.changes.explorer_rows();
    let items: Vec<ListItem<'_>> = if rows.is_empty() {
        vec![ListItem::new(Line::styled(
            " No repository files",
            Style::default().fg(palette().faint),
        ))]
    } else {
        rows.iter()
            .enumerate()
            .skip(app.changes.explorer_scroll)
            .take(viewport)
            .map(|(index, row)| {
                let item = explorer_item(row, usize::from(list_area.width));
                if app.changes.explorer_state.selected() == Some(index) {
                    item.style(Style::default().bg(palette().selected))
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
        "  w:on"
    } else {
        "  w:off"
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
    let lines = styled_source(
        &app.changes.diff,
        app.selected_explorer_file_path().unwrap_or_default(),
        usize::from(preview_body.width),
    );
    render_scrollable_content(frame, app, columns[1], preview_body, lines);
}

fn render_scrollable_content(
    frame: &mut Frame<'_>,
    app: &mut App,
    panel: Rect,
    body: Rect,
    lines: Vec<Line<'static>>,
) {
    let rendered_height =
        rendered_text_height(&lines, usize::from(body.width), app.changes.diff_wrap);
    let viewport_height = usize::from(body.height);
    let max_scroll = rendered_height
        .saturating_sub(viewport_height)
        .min(usize::from(u16::MAX)) as u16;
    app.regions.diff_scroll_max = max_scroll;
    app.changes.diff_scroll = app.changes.diff_scroll.min(max_scroll);
    let scrollbar = Rect::new(panel.right().saturating_sub(1), body.y, 1, body.height);
    app.regions.diff_scrollbar = Some(scrollbar);
    app.regions.diff_scroll_thumb = (max_scroll > 0).then(|| {
        diff_scroll_thumb(
            scrollbar,
            rendered_height,
            viewport_height,
            app.changes.diff_scroll,
            max_scroll,
        )
    });
    let mut paragraph = Paragraph::new(lines)
        .scroll((app.changes.diff_scroll, 0))
        .style(Style::default().bg(palette().panel));
    if app.changes.diff_wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
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

fn worktree_item<'a>(row: &'a WorktreeRow, changes: &'a [Change], width: usize) -> ListItem<'a> {
    let Some(change_index) = row.change_index else {
        let marker = if row.directory_expanded == Some(false) {
            "▸ "
        } else {
            "▾ "
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

fn explorer_item(row: &ExplorerRow, width: usize) -> ListItem<'static> {
    if row.file_index.is_none() {
        let marker = if row.directory_expanded == Some(false) {
            "▸ "
        } else {
            "▾ "
        };
        return ListItem::new(Line::styled(
            truncate_width(&format!("{}{}{}", row.prefix, marker, row.label), width),
            folder_style(),
        ));
    }
    ListItem::new(Line::styled(
        truncate_width(&format!("{}{}", row.prefix, row.label), width),
        Style::default().fg(palette().ink),
    ))
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
    scroll: u16,
    max_scroll: u16,
) -> Rect {
    let thumb_height = (usize::from(track.height) * viewport_height)
        .checked_div(content_height.max(1))
        .unwrap_or(0)
        .max(1)
        .min(usize::from(track.height)) as u16;
    let travel = track.height.saturating_sub(thumb_height);
    let offset = if max_scroll == 0 {
        0
    } else {
        ((u32::from(scroll) * u32::from(travel) + u32::from(max_scroll) / 2)
            / u32::from(max_scroll)) as u16
    };
    Rect::new(
        track.x,
        track.y.saturating_add(offset),
        track.width,
        thumb_height,
    )
}
