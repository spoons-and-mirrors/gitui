mod changes;
mod history;
mod overlays;
mod text;

#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Mode, Regions, View},
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
        View::Changes => changes::draw(frame, app, content),
        View::Graph => {
            app.regions.graph_table =
                history::draw_graph(frame, app.session.data(), &mut app.graph_state, content);
        }
    }
    match app.mode {
        Mode::Picker => {
            let regions = overlays::draw_picker(frame, &mut app.picker);
            app.regions.picker_overlay = Some(regions.overlay);
            app.regions.picker_path = Some(regions.path);
            app.regions.picker_list = Some(regions.list);
        }
        Mode::Settings => {
            let regions = overlays::draw_settings(
                frame,
                &app.settings,
                app.settings_selection,
                app.fetch_running(),
            );
            app.regions.settings_overlay = Some(regions.overlay);
            app.regions.auto_fetch = Some(regions.auto_fetch);
            app.regions.fetch_interval = Some(regions.fetch_interval);
            app.regions.fetch_interval_down = Some(regions.fetch_interval_down);
            app.regions.fetch_interval_up = Some(regions.fetch_interval_up);
        }
        Mode::Help => overlays::draw_help(frame),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (path, branch) = app.repository().map_or_else(
        || ("No repository selected".to_owned(), "offline".to_owned()),
        |repo| (repo.root.display().to_string(), repo.branch.clone()),
    );
    let (staged, unstaged) = app.change_counts();
    let commits = app.repository().map_or(0, |repo| repo.commits.len());

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
