use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::Mode,
    git::{Commit, RepositoryData},
};

use super::{draw_empty, fill, palette, truncate_width};

pub(super) fn draw_graph(
    frame: &mut Frame<'_>,
    repo: Option<&RepositoryData>,
    state: &mut TableState,
    area: Rect,
) -> Option<Rect> {
    let Some(repo) = repo else {
        draw_empty(frame, area, "Open a repository to inspect its graph");
        return None;
    };
    if repo.commits.is_empty() {
        draw_empty(frame, area, "This repository has no commits yet");
        return None;
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
    let graph_region = Rect::new(
        table_area.x,
        table_area.y.saturating_add(2),
        table_area.width,
        table_area.height.saturating_sub(2),
    );

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
            Constraint::Length(16),
            Constraint::Length(16),
            Constraint::Length(9),
        ]
    };

    let viewport = usize::from(graph_region.height);
    let selected = state.selected();
    let mut offset = state
        .offset()
        .min(repo.commits.len().saturating_sub(viewport));
    if let Some(selected) = selected {
        if selected < offset {
            offset = selected;
        } else if selected >= offset.saturating_add(viewport) {
            offset = selected.saturating_add(1).saturating_sub(viewport);
        }
    }
    *state.offset_mut() = offset;
    let rows = repo
        .commits
        .iter()
        .skip(offset)
        .take(viewport)
        .map(|commit| graph_row(commit, compact));
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
    let mut visible_state = TableState::default();
    visible_state.select(selected.and_then(|selected| selected.checked_sub(offset)));
    frame.render_stateful_widget(table, table_area, &mut visible_state);
    Some(graph_region)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_branch(
    frame: &mut Frame<'_>,
    commits: &[Commit],
    branch: &str,
    header: Rect,
    list: Rect,
    dragging: bool,
    focused: bool,
    mode: Mode,
    state: &mut ListState,
) {
    fill(
        frame,
        header,
        if dragging {
            palette().selected
        } else {
            palette().surface_alt
        },
    );
    let history_title = if header.width >= 20 {
        format!("HISTORY  {branch}")
    } else {
        "HISTORY".to_owned()
    };
    let history_meta = format!("↕  {}", commits.len());
    let history_padding = usize::from(header.width).saturating_sub(
        UnicodeWidthStr::width(history_title.as_str())
            + UnicodeWidthStr::width(history_meta.as_str()),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                truncate_width(
                    &history_title,
                    usize::from(header.width)
                        .saturating_sub(UnicodeWidthStr::width(history_meta.as_str()) + 1),
                ),
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(history_padding)),
            Span::styled(history_meta, Style::default().fg(palette().faint)),
        ])),
        header,
    );
    let items: Vec<ListItem<'_>> = if commits.is_empty() {
        vec![ListItem::new(Line::styled(
            "  No commits on this branch",
            Style::default().fg(palette().faint),
        ))]
    } else {
        commits
            .iter()
            .map(|commit| history_item(commit, usize::from(list.width)))
            .collect()
    };
    let history =
        List::new(items).highlight_style(Style::default().bg(if focused && mode == Mode::Normal {
            palette().selected
        } else {
            palette().inactive_selected
        }));
    frame.render_stateful_widget(history, list, state);
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

fn history_item(commit: &Commit, width: usize) -> ListItem<'static> {
    let short_oid: String = commit.oid.chars().take(7).collect();
    let subject_width = width.saturating_sub(8);
    let subject = truncate_width(&commit.subject, subject_width);
    let subject_padding = width.saturating_sub(
        UnicodeWidthStr::width(subject.as_str()) + UnicodeWidthStr::width(short_oid.as_str()),
    );
    let mut details = Vec::new();
    let has_head = commit
        .refs
        .iter()
        .any(|reference| reference == "HEAD" || reference.starts_with("HEAD -> "));
    if has_head {
        details.push(ref_badge("HEAD", palette().green));
    }
    for reference in &commit.refs {
        if reference == "HEAD" || reference.starts_with("HEAD -> ") {
            continue;
        }
        let (label, color) = if let Some(tag) = reference.strip_prefix("tag: ") {
            (tag, palette().yellow)
        } else if reference.contains('/') {
            (reference.as_str(), palette().purple)
        } else {
            (reference.as_str(), palette().accent)
        };
        details.push(ref_badge(label, color));
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(subject, Style::default().fg(palette().ink)),
        Span::raw(" ".repeat(subject_padding)),
        Span::styled(short_oid, Style::default().fg(palette().faint)),
    ])];
    if !details.is_empty() {
        lines.push(Line::from(details));
    }
    ListItem::new(Text::from(lines))
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
