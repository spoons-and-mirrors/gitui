mod changes;
mod history;
mod overlays;
pub(crate) mod preview;
mod text;

#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, FileDialogKind, GraphHitTarget, HitTarget, Mode, Regions, View},
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
            Paragraph::new("hunkle needs at least 60 columns and 16 rows\n\nq  quit")
                .alignment(Alignment::Center)
                .style(Style::default().fg(palette().ink)),
            frame.area(),
        );
        finish_selection(frame, app);
        return;
    }

    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(frame.area());

    app.regions = Regions::default();
    app.regions.screen = Some(frame.area());
    draw_header(frame, app, layout[0]);
    let content = layout[1];
    changes::draw(frame, app, content);
    if app.view == View::Graph && !app.graph_commit_open {
        let graph_area = app.regions.diff.unwrap_or(content);
        frame.render_widget(Clear, graph_area);
        app.regions.diff = None;
        app.regions.diff_scrollbar = None;
        app.regions.diff_scroll_thumb = None;
        app.regions.diff_scroll_max = 0;
        app.regions.diff_hunks.clear();
        let graph_regions = history::draw_graph(
            frame,
            app.session.data(),
            &app.commit_summaries,
            &app.author_filter,
            &mut app.graph_state,
            &mut app.graph_scroll_to_selection,
            graph_area,
        );
        app.regions.graph_table = graph_regions.table;
        for (target, rect) in graph_regions.targets {
            app.regions.register_hit_target(target, rect);
        }
    }
    draw_navigation(frame, app, layout[2]);
    match app.mode {
        Mode::FileSearch => {
            dim(frame);
            let files = app
                .session
                .data()
                .map_or(&[][..], |repo| repo.files.as_slice());
            let regions = overlays::draw_file_search(frame, &mut app.file_search, files);
            app.regions.file_search_overlay = Some(regions.overlay);
            app.regions.file_search_list = Some(regions.list);
        }
        Mode::Explorer => {
            dim(frame);
            let regions = overlays::draw_explorer(frame, &mut app.workspace_explorer);
            app.regions.workspace_explorer_overlay = Some(regions.overlay);
            app.regions.workspace_explorer_path = Some(regions.path);
            app.regions.workspace_explorer_list = Some(regions.list);
        }
        Mode::Settings => {
            dim(frame);
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
            app.regions.editor_setting = Some(regions.editor);
        }
        Mode::RepositoryBrowser => {
            dim(frame);
            for (target, rect) in
                overlays::draw_repository_browser(frame, &mut app.repository_browser)
            {
                app.regions.register_hit_target(target, rect);
            }
        }
        Mode::AuthorFilter => {
            let anchor = app
                .regions
                .hit_target_rect(HitTarget::Graph(GraphHitTarget::AuthorHeader))
                .unwrap_or(Rect::new(content.x, content.y, 1, 1));
            for (target, rect) in history::draw_author_filter(frame, anchor, &mut app.author_filter)
            {
                app.regions.register_hit_target(target, rect);
            }
        }
        Mode::ActionMenu => {
            let anchor = app.regions.actions.unwrap_or(Rect::new(
                content.x.saturating_add(1),
                content.y,
                1,
                1,
            ));
            let regions = overlays::draw_action_menu(frame, anchor, app.actions.selection);
            app.regions.action_menu = Some(regions.overlay);
            app.regions.action_list = Some(regions.list);
        }
        Mode::Command => {
            dim(frame);
            let regions = overlays::draw_command(frame, &mut app.actions);
            app.regions.command_overlay = Some(regions.overlay);
            app.regions.command_output = Some(regions.output);
        }
        Mode::Editor => {
            dim(frame);
            app.regions.editor_overlay = Some(overlays::draw_editor(
                frame,
                &app.editor_input,
                app.editor_error.as_deref(),
                app.editor_configure_only,
            ));
        }
        Mode::Files => {
            if let Some(dialog) = &app.file_dialog {
                let regions = if matches!(dialog.kind, FileDialogKind::Add { .. }) {
                    let anchor = app.regions.files_add.unwrap_or(Rect::new(
                        content.right().saturating_sub(1),
                        content.y,
                        1,
                        1,
                    ));
                    overlays::draw_file_add_popover(frame, anchor, dialog.choice)
                } else {
                    dim(frame);
                    overlays::draw_file_dialog(frame, dialog)
                };
                app.regions.file_dialog_overlay = Some(regions.overlay);
                app.regions.file_dialog_primary = Some(regions.primary);
                app.regions.file_dialog_secondary = Some(regions.secondary);
            }
        }
        Mode::Help => {
            dim(frame);
            overlays::draw_help(frame);
        }
        _ => {}
    }
    finish_selection(frame, app);
}

fn finish_selection(frame: &mut Frame<'_>, app: &mut App) {
    if app.selection.needs_capture(frame.area()) {
        app.selection.capture(frame.buffer_mut());
    }
    app.selection.render(
        frame.buffer_mut(),
        Style::default().fg(palette().canvas).bg(palette().accent),
    );
    app.selection.discard_inactive_capture();
}

fn dim(frame: &mut Frame<'_>) {
    let area = frame.area();
    frame.buffer_mut().set_style(
        area,
        Style::default()
            .bg(Color::Rgb(0, 0, 0))
            .add_modifier(Modifier::DIM),
    );
}

fn draw_header(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (path, branch) = app.repository().map_or_else(
        || ("No repository selected".to_owned(), "offline".to_owned()),
        |repo| (repo.root.display().to_string(), repo.branch.clone()),
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().surface_alt)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let repository = std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("hunkle");
    let branch_label = format!(" {branch} ");
    let branch_width = UnicodeWidthStr::width(branch_label.as_str())
        .min(usize::from(area.width.saturating_sub(12)));
    let notice_label = app
        .notice
        .as_ref()
        .map_or_else(String::new, |notice| format!("  {notice}"));
    let fixed_width = UnicodeWidthStr::width(repository)
        .saturating_add(UnicodeWidthStr::width(notice_label.as_str()))
        .saturating_add(4);
    let left_width = usize::from(area.width).saturating_sub(branch_width);
    let display_path = truncate_width(&path, left_width.saturating_sub(fixed_width));
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
    ];
    if !notice_label.is_empty() {
        title.push(Span::styled(
            notice_label,
            Style::default().fg(palette().yellow),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(title)),
        Rect::new(area.x, area.y, left_width as u16, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            truncate_width(&branch_label, branch_width),
            Style::default()
                .fg(palette().accent)
                .bg(palette().surface_alt)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right),
        Rect::new(
            area.right().saturating_sub(branch_width as u16),
            area.y,
            branch_width as u16,
            1,
        ),
    );
}

fn draw_navigation(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (staged, unstaged) = app.change_counts();
    let commits = app.repository().map_or(0, |repo| repo.commits.len());

    frame.render_widget(
        Block::default().style(Style::default().bg(palette().surface_alt)),
        area,
    );

    let changes_label = format!(" 1 Changes {}/{} ", staged, unstaged);
    let graph_label = format!(" 2 Graph {commits} ");
    let compact = area.width < 88;
    let refresh_label = if compact { " r " } else { " r Refresh " };
    let explorer_label = if compact { " o " } else { " o Explorer " };
    let browser_label = if compact { " b " } else { " b Branches " };
    let settings_label = if compact { " s " } else { " s Settings " };
    let help_label = if compact { " ? " } else { " ? Help " };
    let labels = [
        changes_label.as_str(),
        graph_label.as_str(),
        refresh_label,
        explorer_label,
        browser_label,
        settings_label,
        help_label,
    ];

    let total_width = labels.iter().fold(0_u16, |width, label| {
        width.saturating_add(UnicodeWidthStr::width(*label) as u16)
    });
    let mut spans = Vec::new();
    let start_x = area.right().saturating_sub(total_width).max(area.x);
    let mut x = start_x;
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
        rects.push(Rect::new(x, area.y, width, 1));
        x = x.saturating_add(width);
    }

    app.regions.changes = rects.first().copied();
    app.regions.graph = rects.get(1).copied();
    app.regions.refresh = rects.get(2).copied();
    app.regions.explorer = rects.get(3).copied();
    app.regions.repository_browser = rects.get(4).copied();
    app.regions.settings = rects.get(5).copied();
    app.regions.help = rects.get(6).copied();

    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(start_x, area.y, area.right().saturating_sub(start_x), 1),
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
