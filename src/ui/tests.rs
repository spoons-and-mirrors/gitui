use std::{fs, process::Command, thread, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};

use crate::app::{
    App, BrowserTab, GraphHitTarget, HitTarget, LeftPane, Mode, PullRequest, RemoteItems,
    RepositoryBrowserHitTarget, Settings, View, WorkspacePanel, WorkspacePanelHitTarget,
};

use super::draw;

fn assert_black_underlay(terminal: &Terminal<TestBackend>) {
    let background = &terminal.backend().buffer()[(0, 0)];
    assert_eq!(background.bg, Color::Rgb(0, 0, 0));
    assert!(background.modifier.contains(Modifier::DIM));
}

#[test]
fn renders_every_primary_surface() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    run_git(root, &["config", "user.name", "Render Test"]);
    run_git(root, &["config", "user.email", "render@example.com"]);
    fs::write(root.join("tracked.txt"), "first\n").unwrap();
    fs::create_dir(root.join("fixtures")).unwrap();
    for index in 0..40 {
        fs::write(
            root.join(format!("fixtures/file-{index:02}.txt")),
            format!("fixture {index}\n"),
        )
        .unwrap();
    }
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial commit"]);
    fs::write(root.join("second.txt"), "second\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(
        root,
        &[
            "-c",
            "user.name=Second Author",
            "-c",
            "user.email=second@example.com",
            "commit",
            "-m",
            "second commit",
        ],
    );
    fs::write(root.join("tracked.txt"), "changed\n").unwrap();
    fs::write(root.join("untracked.txt"), "new\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    assert_eq!(app.changes.history_state.selected(), None);
    let settings_path = root.join(".git/hunkle-test-config");
    app.settings_path = Some(settings_path.clone());
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.worktree.unwrap().x, 0);
    assert_eq!(app.regions.worktree.unwrap().y, 1);
    assert_eq!(app.regions.diff.unwrap().right(), 120);
    assert_eq!(app.regions.changes.unwrap().y, 35);
    assert_eq!(app.regions.help.unwrap().y, 35);
    assert!(app.regions.changes.unwrap().x > 0);
    assert_eq!(app.regions.help.unwrap().right(), 120);
    assert!(app.regions.repository_browser.is_some());
    let buffer = terminal.backend().buffer();
    let history = app.regions.history_splitter.unwrap();
    let history_offset = usize::from(history.y) * 120 + usize::from(history.x);
    assert_eq!(buffer.content[0].bg, super::palette().surface_alt);
    assert_eq!(
        buffer.content[36 * 120 - 1].bg,
        super::palette().surface_alt
    );
    assert_eq!(
        buffer.content[history_offset].bg,
        super::palette().surface_alt
    );
    let header: String = terminal.backend().buffer().content[..120]
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(header.trim_end().ends_with("main"));
    let footer: String = terminal.backend().buffer().content[35 * 120..]
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(footer.contains("b Branches"));

    let files_tab = app.regions.files_tab.unwrap();
    click(&mut app, files_tab.x, files_tab.y);
    assert_eq!(app.changes.pane, LeftPane::Files);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.commit.is_none());
    assert!(app.regions.history_list.is_none());
    let mut explorer = app.regions.explorer_list.unwrap();
    let directory_row = app
        .changes
        .explorer_rows()
        .iter()
        .position(|row| row.directory_path.as_deref() == Some("fixtures"))
        .unwrap();
    assert_eq!(
        app.changes.explorer_rows()[directory_row].directory_expanded,
        Some(false)
    );
    click(&mut app, explorer.x + 2, explorer.y + directory_row as u16);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    explorer = app.regions.explorer_list.unwrap();
    let explorer_rows = app.changes.explorer_rows();
    let repo = app.repository().unwrap();
    let selected_file_row = explorer_rows
        .iter()
        .position(|row| row.file_index.is_some())
        .unwrap();
    let selected_file = explorer_rows[selected_file_row]
        .file_index
        .and_then(|index| repo.files.get(index))
        .unwrap()
        .clone();
    click(
        &mut app,
        explorer.x + 2,
        explorer.y + selected_file_row as u16,
    );
    wait_for_preview(&mut app);
    assert_eq!(
        app.selected_explorer_file_path(),
        Some(selected_file.as_str())
    );
    assert_eq!(
        app.changes.diff,
        fs::read_to_string(root.join(&selected_file)).unwrap()
    );
    let selected_before_scroll = app.changes.explorer_state.selected();
    let preview_before_scroll = app.changes.diff.clone();
    app.handle_mouse(mouse(
        MouseEventKind::ScrollDown,
        explorer.x + 2,
        explorer.y + 2,
    ));
    assert_eq!(app.changes.explorer_scroll, 3);
    assert_eq!(
        app.changes.explorer_state.selected(),
        selected_before_scroll
    );
    assert_eq!(app.changes.diff, preview_before_scroll);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let visible_file = app.changes.explorer_rows()[app.changes.explorer_scroll..]
        .iter()
        .position(|row| row.file_index.is_some())
        .unwrap();
    click(&mut app, explorer.x + 2, explorer.y + visible_file as u16);
    assert_ne!(
        app.changes.explorer_state.selected(),
        selected_before_scroll
    );
    let file_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(file_screen.contains("FILE"));
    assert!(file_screen.contains("read-only"));
    assert!(file_screen.contains("fixture"));

    let worktree_tab = app.regions.worktree_tab.unwrap();
    click(&mut app, worktree_tab.x, worktree_tab.y);
    assert_eq!(app.changes.pane, LeftPane::Worktree);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let stage_all = app.regions.stage_all.unwrap();
    assert_eq!(stage_all.width, 2);
    click(&mut app, stage_all.x, stage_all.y);
    wait_for(&mut app, |app| {
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| change.staged)
    });
    assert!(
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| change.staged)
    );
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let staged_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(staged_screen.contains('◉'));
    for (index, cell) in terminal.backend().buffer().content.iter().enumerate() {
        if cell.symbol() == "◉" {
            let trailing = &terminal.backend().buffer().content[index + 1];
            assert_eq!(trailing.symbol(), " ");
            assert_eq!(cell.bg, trailing.bg);
        }
    }
    let stage_all = app.regions.stage_all.unwrap();
    click(&mut app, stage_all.x, stage_all.y);
    wait_for(&mut app, |app| {
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| !change.staged)
    });
    assert!(
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| !change.staged)
    );

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let unstaged_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(unstaged_screen.contains('○'));
    assert!(!unstaged_screen.contains("[ ]"));
    let status = app.regions.worktree_status.unwrap();
    assert_eq!(status.width, 2);
    let selected = app.changes.worktree_state.selected().unwrap();
    let selected_y = status.y + (selected - app.changes.worktree_scroll) as u16;
    click(&mut app, status.x, selected_y);
    wait_for(&mut app, |app| {
        app.repository()
            .unwrap()
            .changes
            .iter()
            .filter(|change| change.staged)
            .count()
            == 1
    });
    assert_eq!(
        app.repository()
            .unwrap()
            .changes
            .iter()
            .filter(|change| change.staged)
            .count(),
        1
    );
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let rows = app.changes.worktree_rows(app.repository().unwrap());
    assert!(rows.iter().any(|row| row.label == "STAGED"));
    assert!(rows.iter().any(|row| row.label == "UNSTAGED"));
    let status = app.regions.worktree_status.unwrap();
    let selected = app.changes.worktree_state.selected().unwrap();
    let selected_y = status.y + (selected - app.changes.worktree_scroll) as u16;
    click(&mut app, status.x, selected_y);
    wait_for(&mut app, |app| {
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| !change.staged)
    });
    assert!(
        app.repository()
            .unwrap()
            .changes
            .iter()
            .all(|change| !change.staged)
    );

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let worktree = app.regions.worktree_list.unwrap();
    let tracked_row = app
        .changes
        .worktree_rows(app.repository().unwrap())
        .iter()
        .position(|row| row.label == "tracked.txt")
        .unwrap();
    let tracked_y = worktree.y + (tracked_row - app.changes.worktree_scroll) as u16;
    click(&mut app, worktree.x + 10, tracked_y);
    assert_eq!(app.changes.worktree_state.selected(), Some(tracked_row));

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
        fs::read_to_string(&settings_path)
            .unwrap()
            .contains("worktree_width=65")
    );
    assert!(!app.dragging_splitter);

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let history_splitter = app.regions.history_splitter.unwrap();
    let commit = app.regions.commit.unwrap();
    let actions = app.regions.actions.unwrap();
    let worktree = app.regions.worktree_list.unwrap();
    assert_eq!(actions.y, commit.bottom());
    assert_eq!(actions.right(), commit.right());
    assert_eq!(actions.bottom(), worktree.y);
    assert!(commit.bottom() <= history_splitter.y);
    let history_bounds = app.regions.history_bounds.unwrap();
    let history_target = history_bounds.bottom().saturating_sub(9);
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        history_splitter.right().saturating_sub(2),
        history_splitter.y,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        history_splitter.right().saturating_sub(2),
        history_target,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        history_splitter.right().saturating_sub(2),
        history_target,
    ));
    assert_eq!(app.settings.history_height, 9);
    assert!(
        fs::read_to_string(&settings_path)
            .unwrap()
            .contains("history_height=9")
    );
    assert!(!app.dragging_history);

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let history = app.regions.history_list.unwrap();
    click(&mut app, history.x + 2, history.y + 2);
    wait_for_preview(&mut app);
    assert_eq!(app.changes.history_state.selected(), Some(1));
    assert!(app.changes.history_focused);
    assert!(app.changes.diff.contains("diff --git"));

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let worktree = app.regions.worktree_list.unwrap();
    let tracked_row = app
        .changes
        .worktree_rows(app.repository().unwrap())
        .iter()
        .position(|row| row.label == "tracked.txt")
        .unwrap();
    let tracked_y = worktree.y + (tracked_row - app.changes.worktree_scroll) as u16;
    click(&mut app, worktree.x + 2, tracked_y);
    wait_for_preview(&mut app);
    assert_eq!(app.changes.history_state.selected(), None);
    assert!(!app.changes.history_focused);
    assert!(app.changes.diff.contains("tracked.txt"));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let diff = app.regions.diff.unwrap();
    let summary_row: String = (diff.x..diff.right())
        .map(|column| terminal.backend().buffer()[(column, diff.y + 3)].symbol())
        .collect();
    let files_row: String = (diff.x..diff.right())
        .map(|column| terminal.backend().buffer()[(column, diff.y + 4)].symbol())
        .collect();
    assert!(summary_row.contains("CHANGES"));
    assert!(summary_row.contains("+1"));
    assert!(summary_row.contains("-1"));
    assert!(files_row.contains("FILES"));
    assert!(files_row.contains("tracked.txt"));
    let tracked_diff = app.changes.diff.clone();
    app.changes.set_diff(
        concat!(
            "diff --git a/tracked.txt b/tracked.txt\n",
            "--- a/tracked.txt\n",
            "+++ b/tracked.txt\n",
            "@@ -1 +1 @@\n-old one\n+new one\n",
            "@@ -3 +3 @@\n-old two\n+new two\n",
        )
        .to_owned(),
    );
    app.changes.diff_scroll = 0;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let normal_hunk_y = app.regions.diff_hunks[0].action.unwrap().y;
    let normal_scroll_max = app.regions.diff_scroll_max;
    let normal_scroll_thumb = app.regions.diff_scroll_thumb;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(0));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.diff_hunks[0].action.unwrap().y, normal_hunk_y);
    assert_eq!(app.regions.diff_scroll_max, normal_scroll_max);
    assert_eq!(app.regions.diff_scroll_thumb, normal_scroll_thumb);
    assert_eq!(app.regions.diff_hunks.len(), 2);
    let pinned_hunk_y = app.regions.diff_hunks[0].action.unwrap().y;
    let second_hunk = app.regions.diff_hunks[1].rect;
    app.handle_mouse(mouse(
        MouseEventKind::Moved,
        second_hunk.x + 1,
        second_hunk.y,
    ));
    assert_eq!(app.changes.hunk_selection, Some(1));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let selected_hunk = app
        .regions
        .diff_hunks
        .iter()
        .find(|hunk| hunk.index == 1)
        .unwrap();
    assert_eq!(selected_hunk.action.unwrap().y, pinned_hunk_y);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(1));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, None);
    app.changes.set_diff(format!(
        "@@ -1,80 +1,80 @@\n{}",
        (0..80)
            .map(|line| format!(" line {line}\n"))
            .collect::<String>()
    ));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.diff_hunks[0].continues_below);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(0));
    assert_eq!(app.changes.diff_scroll, 10);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.changes.diff_scroll, 10);
    assert!(app.regions.diff_hunks[0].continues_above);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.changes.diff_scroll, 0);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.changes.set_diff(tracked_diff);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(0));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.diff_hunks.len(), 1);
    let rect = app.regions.diff_hunks[0].action.unwrap();
    let buffer = terminal.backend().buffer();
    let offset = usize::from(rect.y) * usize::from(buffer.area.width) + usize::from(rect.x);
    let button: String = buffer.content[offset..offset + 3]
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert_eq!(button, "[+]");
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.changes.hunk_selection, Some(0));
    wait_for(&mut app, |app| {
        app.repository()
            .unwrap()
            .changes
            .iter()
            .any(|change| change.path == "tracked.txt" && change.staged)
    });
    let rows = app.changes.worktree_rows(app.repository().unwrap());
    assert!(rows.iter().any(|row| row.label == "STAGED"));
    assert!(rows.iter().any(|row| row.label == "UNSTAGED"));

    app.changes.set_diff(
        (0..100)
            .map(|line| format!("+scrollbar line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    app.changes.diff_scroll = 0;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let scrollbar = app.regions.diff_scrollbar.unwrap();
    assert_eq!(scrollbar.width, 1);
    assert_eq!(scrollbar.right(), 120);
    assert!(app.regions.diff_scroll_max > 0);
    assert!(app.regions.diff_scroll_thumb.is_some());
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        scrollbar.x,
        scrollbar.bottom() - 1,
    ));
    assert!(app.dragging_diff_scrollbar);
    assert!(app.changes.diff_scroll > 0);
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        scrollbar.x,
        scrollbar.y,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        scrollbar.x,
        scrollbar.y,
    ));
    assert_eq!(app.changes.diff_scroll, 0);
    assert!(!app.dragging_diff_scrollbar);

    app.changes.set_diff(
        (0..30_001)
            .map(|line| format!("+{line:05} {}", "x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    app.changes.diff_wrap = true;
    app.changes.diff_scroll = usize::MAX;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.changes.preview_presentation.is_windowed());
    assert!(app.changes.diff_scroll > usize::from(u16::MAX));
    app.changes.diff_wrap = false;

    let changes_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(changes_screen.contains("Write a commit message"));
    assert!(changes_screen.contains("HISTORY"));
    assert!(changes_screen.contains("ACTIONS"));
    assert!(app.regions.actions.is_some());
    assert!(app.regions.actions.unwrap().bottom() <= app.regions.worktree_list.unwrap().y);
    assert!(changes_screen.contains("HEAD"));
    let history_oid: String = app.repository().unwrap().history[0]
        .oid
        .chars()
        .take(7)
        .collect();
    let history_date = app.repository().unwrap().history[0].date.clone();
    assert!(changes_screen.contains(&history_oid));
    assert!(!changes_screen.contains(&history_date));
    assert!(!changes_screen.contains("Render Test"));
    assert!(!changes_screen.contains('●'));
    assert!(!changes_screen.contains("[Commit]"));
    assert!(!changes_screen.contains("COMMIT"));
    assert!(!changes_screen.contains('┌'));
    let actions = app.regions.actions.unwrap();
    click(&mut app, actions.x + 2, actions.y);
    assert_eq!(app.mode, Mode::ActionMenu);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let action_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(action_screen.contains("Pull --rebase"));
    assert!(action_screen.contains("Commit"));
    assert!(action_screen.contains("Run Git command"));
    let action_list = app.regions.action_list.unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Moved,
        action_list.x + 2,
        action_list.y + 4,
    ));
    assert_eq!(app.actions.selection, 4);
    let background_before_command = terminal.backend().buffer().content[0].clone();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Command);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let command_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(command_screen.contains("GIT COMMAND"));
    assert!(command_screen.contains("Shell pipes"));
    assert!(app.regions.command_output.is_some());
    let command_overlay = app.regions.command_overlay.unwrap();
    let command_output = app.regions.command_output.unwrap();
    assert_eq!(
        command_output.bottom().saturating_add(1),
        command_overlay.bottom().saturating_sub(5)
    );
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    let background = &buffer.content[0];
    let modal =
        &buffer.content[usize::from(command_overlay.y) * width + usize::from(command_overlay.x)];
    assert!(background.modifier.contains(Modifier::DIM));
    assert_eq!(background.fg, background_before_command.fg);
    assert_eq!(background.bg, Color::Rgb(0, 0, 0));
    assert!(!modal.modifier.contains(Modifier::DIM));
    assert_eq!(modal.bg, super::palette().surface_alt);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Normal);
    let commit = app.regions.commit.unwrap();
    click(&mut app, commit.x + 2, commit.y + 1);
    assert_eq!(app.mode, Mode::Commit);
    app.commit_input.set("ac");
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(app.commit_input.text(), "abc");
    app.commit_input.set("alpha beta");
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(app.commit_input.text(), "alpha ");
    app.commit_input.set("alpha beta");
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
    assert_eq!(app.commit_input.text(), "alpha ");
    app.commit_input.set("replace me");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    let input_cell =
        &buffer.content[usize::from(commit.y) * width + usize::from(commit.x.saturating_add(1))];
    let focus_edge = &buffer.content[usize::from(commit.y) * width + usize::from(commit.x)];
    assert_eq!(input_cell.bg, super::palette().selected);
    assert_eq!(focus_edge.bg, super::palette().canvas);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.commit_input.text(), "x");
    app.commit_input.set("Subject");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.commit_input.text(), "Subject\n");
    app.commit_input.insert("Body");
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

    app.commit_input
        .set(format!("wrap-start {} wrap-end", "x".repeat(90)));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let wrapped_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(wrapped_screen.contains("wrap-start"));
    assert!(wrapped_screen.contains("wrap-end"));
    app.commit_input.set("Subject\nBody");

    app.mode = Mode::Commit;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let diff = app.regions.diff.unwrap();
    click(&mut app, diff.x + 1, diff.y + 1);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.commit_input.text(), "Subject\nBody");

    app.mode = Mode::Commit;
    app.commit_input.clear();
    app.notice = None;
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    assert_eq!(
        app.notice.as_deref(),
        Some("Commit message cannot be empty")
    );

    app.view = View::Graph;
    app.mode = Mode::Normal;
    let visible_oid = app.repository().unwrap().commits[0].oid.clone();
    wait_for(&mut app, |app| {
        app.commit_summaries.get(&visible_oid).is_some()
    });
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let visible_summary = app.commit_summaries.get(&visible_oid).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("AUTHOR"));
    assert!(screen.contains("CHANGES"));
    assert!(screen.contains(&format!(
        "+{} -{}",
        visible_summary.additions, visible_summary.deletions
    )));
    assert!(screen.contains("HEAD"));
    assert!(screen.contains("Render Test"));
    assert!(screen.contains("WORKTREE"));
    assert!(screen.contains("o Explorer"));
    assert!(!screen.contains("scrollbar line"));
    let worktree = app.regions.worktree.unwrap();
    let mut graph = app.regions.graph_table.unwrap();
    assert!(graph.x >= worktree.right());
    assert!(app.regions.diff.is_none());

    let author_header = app
        .regions
        .hit_target_rect(HitTarget::Graph(GraphHitTarget::AuthorHeader))
        .unwrap();
    click(&mut app, author_header.x, author_header.y);
    assert_eq!(app.mode, Mode::AuthorFilter);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let second_author = app
        .author_filter
        .entries()
        .iter()
        .position(|entry| entry.name == "Second Author")
        .unwrap();
    let second_author_row = app
        .regions
        .hit_target_rect(HitTarget::Graph(GraphHitTarget::FilterItem(second_author)))
        .unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: second_author_row.x + 1,
        row: second_author_row.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.author_filter.state.selected(), Some(second_author));
    click(&mut app, second_author_row.x + 1, second_author_row.y);
    assert_eq!(app.visible_graph_indices().len(), 1);
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.visible_graph_indices().len(), 2);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Normal);

    let branch_history = app.regions.history_list.unwrap();
    click(&mut app, branch_history.x + 1, branch_history.y);
    assert_eq!(app.changes.history_state.selected(), Some(0));
    assert!(app.graph_commit_open);
    wait_for_preview(&mut app);
    assert!(!app.changes.diff.is_empty());
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.graph_table.is_none());
    assert!(app.regions.diff.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    graph = app.regions.graph_table.unwrap();

    let graph_offset = app.graph_state.offset();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: graph.x + 1,
        row: graph.y + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.graph_state.selected(), Some(1));
    assert_eq!(app.graph_state.offset(), graph_offset);
    assert!(!app.graph_commit_open);
    click(&mut app, graph.x + 1, graph.y + 1);
    assert_eq!(app.graph_state.selected(), Some(1));
    assert!(app.graph_commit_open);
    wait_for_preview(&mut app);
    assert!(app.changes.diff.contains("tracked.txt"));
    let commit_oid = app.repository().unwrap().commits[1].oid.clone();
    wait_for(&mut app, |app| {
        app.commit_summaries.get(&commit_oid).is_some()
    });
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let commit_diff_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(commit_diff_screen.contains("DIFF"));
    assert!(commit_diff_screen.contains("initial commit"));
    assert!(commit_diff_screen.contains("CHANGES"));
    assert!(commit_diff_screen.contains("FILES"));
    assert!(
        commit_diff_screen.contains("diff --git a/fixtures/file-00"),
        "the first file heading should be visible"
    );
    let commit_diff = app.regions.diff.unwrap();
    let unwrapped_file_summary: String = (commit_diff.x..commit_diff.right())
        .map(|column| terminal.backend().buffer()[(column, commit_diff.y + 3)].symbol())
        .collect();
    assert!(!unwrapped_file_summary.contains("fixtures/file-05.txt"));
    assert!(app.regions.graph_table.is_none());
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let mut wrapped_file_summary = String::new();
    for row in commit_diff.y + 3..commit_diff.y + 9 {
        for column in commit_diff.x..commit_diff.right() {
            wrapped_file_summary.push_str(terminal.backend().buffer()[(column, row)].symbol());
        }
    }
    assert!(wrapped_file_summary.contains("fixtures/file-05.txt"));
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.view, View::Graph);
    assert!(!app.graph_commit_open);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.graph_table.is_some());
    assert!(app.regions.diff_hunks.is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.graph_state.selected(), Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.graph_commit_open);
    wait_for_preview(&mut app);
    assert!(app.changes.diff.contains("second.txt"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert_eq!(app.workspace_explorer.directory, root);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);
    let explorer_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(explorer_screen.contains("EXPLORER"));
    assert!(explorer_screen.contains("Switch working directory"));
    assert!(explorer_screen.contains("BROWSE"));
    assert!(!explorer_screen.contains("OPEN REPOSITORY"));
    assert!(!explorer_screen.contains('┌'));
    assert!(app.regions.workspace_explorer_list.is_some());
    let path = app.regions.workspace_explorer_path.unwrap();
    click(&mut app, path.x + 2, path.y + 1);
    assert!(app.workspace_explorer.editing_path);

    app.mode = Mode::Normal;
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::RepositoryBrowser);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let browser_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(browser_screen.contains("PULL REQUESTS"));
    assert!(browser_screen.contains("LOCAL & REMOTE"));
    assert!(browser_screen.contains("main"));
    assert!(
        app.regions
            .hit_target_rect(HitTarget::RepositoryBrowser(
                RepositoryBrowserHitTarget::List,
            ))
            .is_some()
    );
    let browser_overlay = app
        .regions
        .hit_target_rect(HitTarget::RepositoryBrowser(
            RepositoryBrowserHitTarget::Overlay,
        ))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let background = &buffer[(0, 0)];
    let modal = &buffer[(browser_overlay.x, browser_overlay.y)];
    assert_eq!(background.bg, Color::Rgb(0, 0, 0));
    assert!(background.modifier.contains(Modifier::DIM));
    assert_eq!(modal.bg, super::palette().surface_alt);
    assert!(!modal.modifier.contains(Modifier::DIM));

    let target_oid = app.repository().unwrap().commits[1].oid.clone();
    let mut target_branch = app.repository_browser.branches[0].clone();
    target_branch.name = "test/hover-target".to_owned();
    target_branch.oid = target_oid;
    target_branch.current = false;
    app.repository_browser.branches.push(target_branch);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let list = app
        .regions
        .hit_target_rect(HitTarget::RepositoryBrowser(
            RepositoryBrowserHitTarget::List,
        ))
        .unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: list.x + 4,
        row: list.y + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.repository_browser.state.selected(), Some(1));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[(list.x + 4, list.y + 1)].bg,
        super::palette().selected
    );
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert!(app.repository_browser.branch_delete_open());
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let delete_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(delete_screen.contains("DELETE BRANCH"));
    assert!(delete_screen.contains("Delete local branch test/hover-target?"));
    assert!(delete_screen.contains("Local only"));
    assert!(delete_screen.contains("Force local"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.repository_browser.branch_delete_open());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.view, View::Graph);
    assert_eq!(app.graph_state.selected(), Some(1));
    assert_eq!(app.graph_state.offset(), 0);
    assert!(!app.graph_scroll_to_selection);

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let list = app
        .regions
        .hit_target_rect(HitTarget::RepositoryBrowser(
            RepositoryBrowserHitTarget::List,
        ))
        .unwrap();
    let branch_oid = app.repository_browser.branches[0].oid.clone();
    let branch_tip = app
        .repository()
        .unwrap()
        .commits
        .iter()
        .position(|commit| commit.oid.starts_with(&branch_oid))
        .unwrap();
    app.graph_state
        .select(Some(usize::from(branch_tip == 0).min(1)));
    click(&mut app, list.x + 4, list.y);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.view, View::Graph);
    assert_eq!(app.graph_state.selected(), Some(branch_tip));
    assert_eq!(app.graph_state.offset(), branch_tip.saturating_sub(5));

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));

    app.repository_browser.pull_requests = RemoteItems::ready(vec![
        PullRequest {
            number: 42,
            title: "Improve browser readability".to_owned(),
            branch: "feature/browser".to_owned(),
            author: "octocat".to_owned(),
            draft: true,
        },
        PullRequest {
            number: 43,
            title: "Polish metadata colors".to_owned(),
            branch: "feature/colors".to_owned(),
            author: "hubot".to_owned(),
            draft: false,
        },
    ]);
    app.repository_browser.set_tab(BrowserTab::PullRequests);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let list = app
        .regions
        .hit_target_rect(HitTarget::RepositoryBrowser(
            RepositoryBrowserHitTarget::List,
        ))
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(list.x + 4, list.y + 1)].fg, super::palette().ink);
    assert_eq!(buffer[(list.x + 4, list.y + 3)].fg, super::palette().cyan);
    click(&mut app, list.x + 4, list.y + 2);
    assert_eq!(app.repository_browser.state.selected(), Some(1));
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert_eq!(app.repository_browser.query, "m");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Normal);

    app.mode = Mode::Settings;
    app.settings = Settings::default();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);
    let settings_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(settings_screen.contains("Auto-fetch remotes"));
    assert!(settings_screen.contains("Fetch interval"));
    assert!(settings_screen.contains("Editor command"));
    assert!(app.regions.editor_setting.is_some());
    assert!(!settings_screen.contains('┌'));
    assert!(app.regions.auto_fetch.is_some());
    assert!(app.regions.fetch_interval_up.is_some());

    app.mode = Mode::Editor;
    app.editor_input = "nvim".to_owned();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);
    let editor_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(editor_screen.contains("EDITOR COMMAND"));
    assert!(editor_screen.contains("nvim"));
    assert!(editor_screen.contains("Saved for next time"));
    assert!(app.regions.editor_overlay.is_some());

    app.mode = Mode::Help;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);
    let help_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(help_screen.contains("KEYBOARD"));
    assert!(help_screen.contains("Ctrl+Enter"));
    assert!(help_screen.contains("Explorer"));
    assert!(!help_screen.contains('┌'));

    let mut narrow = Terminal::new(TestBackend::new(50, 12)).unwrap();
    narrow.draw(|frame| draw(frame, &mut app)).unwrap();
}

#[test]
fn renders_herdr_workspaces_and_agents_as_an_app_level_rail() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path().to_path_buf());
    let settings_path = directory.path().join("config");
    app.settings_path = Some(settings_path.clone());
    app.settings.workspace_panel_width = 26;
    app.workspace_panel = WorkspacePanel::ready_for_test(&serde_json::json!({
        "result": {
            "snapshot": {
                "workspaces": [{
                    "workspace_id": "w1",
                    "label": "HUNKLE",
                    "number": 4,
                    "pane_count": 2,
                    "focused": true,
                    "agent_status": "working"
                }],
                "agents": [{
                    "agent": "opencode",
                    "agent_status": "working",
                    "focused": true,
                    "pane_id": "w1:p1",
                    "tab_id": "w1:t1",
                    "workspace_id": "w1"
                }]
            }
        }
    }));
    app.workspace_panel.workspaces[0].branch = Some("topic".to_owned());
    app.mode = Mode::WorkspacePanel;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    assert_eq!(app.regions.workspace_panel.unwrap().width, 26);
    assert_eq!(app.regions.worktree.unwrap().x, 27);
    assert_eq!(
        app.regions
            .hit_target_rect(HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Collapse,))
            .unwrap()
            .y,
        app.regions.worktree.unwrap().y + 1
    );
    assert!(
        app.regions
            .hit_target_rect(HitTarget::WorkspacePanel(
                WorkspacePanelHitTarget::Workspace(0)
            ))
            .is_some()
    );
    assert!(
        app.regions
            .hit_target_rect(HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Agent(0)))
            .is_some()
    );
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("WORKSPACES  +"));
    assert!(rendered.contains("HUNKLE"));
    assert!(rendered.contains("topic"));
    assert!(rendered.contains("AGENTS"));
    assert!(rendered.contains("opencode / HUNKLE"));
    let workspace_row = app
        .regions
        .hit_target_rect(HitTarget::WorkspacePanel(
            WorkspacePanelHitTarget::Workspace(0),
        ))
        .unwrap();
    let branch_cell = &terminal.backend().buffer()[(workspace_row.right() - 7, workspace_row.y)];
    assert_eq!(branch_cell.symbol(), "t");
    assert_eq!(branch_cell.fg, super::palette().accent);
    assert_eq!(branch_cell.bg, super::palette().selected);
    let create_button = app
        .regions
        .hit_target_rect(HitTarget::WorkspacePanel(
            WorkspacePanelHitTarget::CreateWorkspace,
        ))
        .unwrap();
    assert_eq!(create_button.width, 3);
    assert_eq!(create_button.x, app.regions.workspace_panel.unwrap().x + 12);
    assert_eq!(
        terminal.backend().buffer()[(create_button.x + 1, create_button.y)].bg,
        super::palette().raised
    );
    app.handle_mouse(mouse(
        MouseEventKind::Moved,
        create_button.x + 1,
        create_button.y,
    ));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[(create_button.x + 1, create_button.y)].bg,
        super::palette().accent
    );
    app.mode = Mode::Normal;

    let divider = app.regions.workspace_panel_splitter.unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        divider.x,
        divider.y + 2,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        35,
        divider.y + 2,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        35,
        divider.y + 2,
    ));
    assert_eq!(app.settings.workspace_panel_width, 35);
    assert!(!app.dragging_workspace_panel_splitter);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.workspace_panel.unwrap().width, 35);
    assert_eq!(app.regions.worktree.unwrap().x, 36);

    app.workspace_panel.begin_group();
    app.workspace_panel.paste("Current work");
    app.workspace_panel
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(
        app.regions
            .hit_target_rect(HitTarget::WorkspacePanel(WorkspacePanelHitTarget::Group(0)))
            .is_some()
    );
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Current work"));

    app.workspace_panel.cycle_placement();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.workspace_panel.unwrap().x, 85);
    assert_eq!(app.regions.worktree.unwrap().x, 0);

    let divider = app.regions.workspace_panel_splitter.unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        divider.x,
        divider.y + 2,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        78,
        divider.y + 2,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        78,
        divider.y + 2,
    ));
    assert_eq!(app.settings.workspace_panel_width, 41);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.regions.workspace_panel.unwrap().x, 79);
    assert_eq!(app.regions.workspace_panel.unwrap().width, 41);
    assert!(
        fs::read_to_string(&settings_path)
            .unwrap()
            .contains("workspace_panel_width=41")
    );

    app.workspace_panel.cycle_placement();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.workspace_panel.is_none());
    assert_eq!(app.regions.worktree.unwrap().x, 0);

    app.workspace_panel.show_left();

    let mut narrow = Terminal::new(TestBackend::new(78, 24)).unwrap();
    narrow.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(app.regions.workspace_panel.is_none());
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(
        app.notice.as_deref(),
        Some("Workspaces need a wider terminal")
    );
}

#[test]
fn toggles_worktree_directories_with_the_mouse() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/app.rs"), "fn main() {}\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(
        app.changes.worktree_rows(app.repository().unwrap()).len(),
        4
    );

    let worktree = app.regions.worktree_list.unwrap();
    let directory_row = app
        .changes
        .worktree_rows(app.repository().unwrap())
        .iter()
        .position(|row| row.directory_path.is_some())
        .unwrap();
    let directory_y = worktree.y + (directory_row - app.changes.worktree_scroll) as u16;
    click(&mut app, worktree.x + 1, directory_y);
    assert_eq!(app.changes.worktree_state.selected(), Some(directory_row));
    let rows = app.changes.worktree_rows(app.repository().unwrap());
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].directory_expanded, Some(false));

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let worktree = app.regions.worktree_list.unwrap();
    let directory_row = app
        .changes
        .worktree_rows(app.repository().unwrap())
        .iter()
        .position(|row| row.directory_path.is_some())
        .unwrap();
    let directory_y = worktree.y + (directory_row - app.changes.worktree_scroll) as u16;
    click(&mut app, worktree.x + 1, directory_y);
    assert_eq!(
        app.changes.worktree_rows(app.repository().unwrap()).len(),
        4
    );
}

#[test]
fn colors_changed_files_in_the_files_view() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    run_git(root, &["config", "user.name", "Render Test"]);
    run_git(root, &["config", "user.email", "render@example.com"]);
    fs::write(root.join("modified.txt"), "original\n").unwrap();
    fs::write(root.join("deleted.txt"), "deleted\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial commit"]);

    fs::write(root.join("modified.txt"), "changed\n").unwrap();
    fs::write(root.join("added.txt"), "added\n").unwrap();
    run_git(root, &["add", "added.txt"]);
    fs::write(root.join("new.txt"), "new\n").unwrap();
    run_git(root, &["rm", "deleted.txt"]);
    fs::write(root.join("deleted.txt"), "replacement\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    app.changes.pane = LeftPane::Files;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let list = app.regions.explorer_list.unwrap();
    let rows = app.changes.explorer_rows();
    let repo = app.repository().unwrap();
    for (path, expected) in [
        ("added.txt", super::palette().accent),
        ("deleted.txt", super::palette().red),
        ("modified.txt", super::palette().yellow),
        ("new.txt", super::palette().green),
    ] {
        let row_index = rows
            .iter()
            .position(|row| {
                row.file_index
                    .and_then(|index| repo.files.get(index))
                    .is_some_and(|file| file == path)
            })
            .unwrap();
        let row = &rows[row_index];
        let x = list.x + row.prefix.chars().count() as u16;
        let y = list.y + row_index.saturating_sub(app.changes.explorer_scroll) as u16;
        assert_eq!(terminal.backend().buffer()[(x, y)].fg, expected, "{path}");
    }
}

#[test]
fn colors_collapsed_folders_for_the_changes_they_contain() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    run_git(root, &["config", "user.name", "Render Test"]);
    run_git(root, &["config", "user.email", "render@example.com"]);
    for path in ["modified/file.txt", "deleted/file.txt", "deleted/keep.txt"] {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), "original\n").unwrap();
    }
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial commit"]);

    fs::write(root.join("modified/file.txt"), "changed\n").unwrap();
    fs::create_dir_all(root.join("added")).unwrap();
    fs::write(root.join("added/file.txt"), "added\n").unwrap();
    run_git(root, &["add", "added/file.txt"]);
    fs::create_dir_all(root.join("untracked")).unwrap();
    fs::write(root.join("untracked/file.txt"), "new\n").unwrap();
    run_git(root, &["rm", "deleted/file.txt"]);

    let mut app = App::new(root.to_path_buf());
    app.changes.pane = LeftPane::Files;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let list = app.regions.explorer_list.unwrap();
    let rows = app.changes.explorer_rows();
    for (path, expected) in [
        ("added", super::palette().accent),
        ("deleted", super::palette().red),
        ("modified", super::palette().yellow),
        ("untracked", super::palette().green),
    ] {
        let row_index = rows
            .iter()
            .position(|row| row.directory_path.as_deref() == Some(path))
            .unwrap();
        assert_eq!(rows[row_index].directory_expanded, Some(false));
        let x = list.x + rows[row_index].prefix.chars().count() as u16;
        let y = list.y + row_index.saturating_sub(app.changes.explorer_scroll) as u16;
        assert_eq!(terminal.backend().buffer()[(x, y)].fg, expected, "{path}");
    }
}

#[test]
fn files_click_waits_for_release_without_styling_every_file_as_a_drop_target() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("first.txt"), "first\n").unwrap();
    fs::write(root.join("second.txt"), "second\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let list = app.regions.explorer_list.unwrap();
    let selected_before = app.changes.explorer_state.selected();
    let target = app
        .changes
        .explorer_rows()
        .iter()
        .enumerate()
        .find_map(|(index, row)| {
            (row.file_index.is_some() && Some(index) != selected_before).then_some(index)
        })
        .unwrap();
    let y = list.y + target.saturating_sub(app.changes.explorer_scroll) as u16;

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        list.x + 1,
        y,
    ));
    assert_eq!(app.changes.explorer_state.selected(), selected_before);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    for (index, row) in app.changes.explorer_rows().iter().enumerate() {
        if row.file_index.is_some() && Some(index) != selected_before {
            let y = list.y + index.saturating_sub(app.changes.explorer_scroll) as u16;
            assert_ne!(
                terminal.backend().buffer()[(list.x, y)].bg,
                super::palette().inactive_selected
            );
        }
    }

    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), list.x + 1, y));
    assert_eq!(app.changes.explorer_state.selected(), Some(target));
}

#[test]
fn opens_plain_directories_as_file_workspaces() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("config/nested")).unwrap();
    fs::write(root.join("README.md"), "local workspace\n").unwrap();
    fs::write(root.join("config/nested/settings.toml"), "theme = 'test'\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.repository().unwrap().is_local());
    assert_eq!(app.changes.pane, LeftPane::Files);
    wait_for_preview(&mut app);
    assert_eq!(app.changes.diff, "local workspace\n");

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("WORKTREE"));
    assert!(screen.contains("FILES"));
    assert!(screen.contains("README.md"));
    assert!(screen.contains("local workspace"));

    let add = app.regions.files_add.unwrap();
    click(&mut app, add.x, add.y);
    assert_eq!(app.mode, Mode::Files);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let popover = app.regions.file_dialog_overlay.unwrap();
    assert_eq!(popover.y, add.bottom());
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!screen.contains("ADD TO FILES"));
    assert!(screen.contains("New file"));
    assert!(screen.contains("New folder"));
    let new_file = app.regions.file_dialog_primary.unwrap();
    click(&mut app, new_file.x, new_file.y);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("NEW FILE"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    let worktree_tab = app.regions.worktree_tab.unwrap();
    click(&mut app, worktree_tab.x, worktree_tab.y);
    assert_eq!(app.changes.pane, LeftPane::Worktree);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("Working tree clean"));
    assert!(screen.contains("LOCAL WORKSPACE"));
    assert!(screen.contains("Local file workspace"));
}

#[test]
fn fuzzy_searches_and_opens_repository_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::write(
        root.join("src/components/profile_card.rs"),
        "pub struct ProfileCard;\n",
    )
    .unwrap();
    fs::write(
        root.join("src/components/button.rs"),
        "pub struct Button;\n",
    )
    .unwrap();
    fs::write(root.join("README.md"), "search fixture\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
    assert_eq!(app.mode, Mode::FileSearch);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.view = View::Graph;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
    for character in "profile card".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    assert_eq!(app.mode, Mode::FileSearch);
    assert_eq!(app.file_search.match_count, 1);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_black_underlay(&terminal);

    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("FIND FILE"));
    assert!(screen.contains("profile_card.rs"));
    assert!(screen.contains("src/components"));
    assert!(app.regions.file_search_overlay.is_some());
    assert!(app.regions.file_search_list.is_some());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    wait_for_preview(&mut app);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.view, View::Changes);
    assert_eq!(app.changes.pane, LeftPane::Files);
    assert_eq!(
        app.selected_explorer_file_path(),
        Some("src/components/profile_card.rs")
    );
    assert_eq!(app.changes.diff, "pub struct ProfileCard;\n");
    assert!(
        app.changes
            .explorer_rows()
            .iter()
            .any(|row| row.label == "profile_card.rs")
    );
}

#[test]
fn selects_visible_text_and_suppresses_clicks_after_dragging() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    run_git(root, &["init", "-b", "main"]);
    fs::write(root.join("selected.txt"), "select me\n").unwrap();

    let mut app = App::new(root.to_path_buf());
    app.changes.set_diff("select me".to_owned());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let diff = app.regions.diff.unwrap();
    let buffer = terminal.backend().buffer();
    let start = (diff.y..diff.bottom())
        .find_map(|row| {
            let text: String = (diff.x..diff.right())
                .map(|column| buffer[(column, row)].symbol())
                .collect();
            text.find("select")
                .map(|column| (diff.x + column as u16, row))
        })
        .expect("rendered preview should contain selectable text");
    let end = (start.0 + 5, start.1);
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        start.0,
        start.1,
    ));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), end.0, end.1));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    let index = usize::from(start.1) * usize::from(buffer.area.width) + usize::from(start.0);
    assert_eq!(buffer.content[index].bg, super::palette().accent);

    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), end.0, end.1));
    assert_eq!(app.take_copy_request().as_deref(), Some("select"));

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let graph = app.regions.graph.unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        graph.x + 2,
        graph.y,
    ));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        graph.x + 4,
        graph.y,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        graph.x + 4,
        graph.y,
    ));
    assert_eq!(app.view, View::Changes);
    assert!(app.take_copy_request().is_some());
}

fn wait_for_preview(app: &mut App) {
    for _ in 0..100 {
        let _ = app.poll_worker();
        if app.changes.diff != "Loading preview…" {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("preview did not complete");
}

fn wait_for(app: &mut App, predicate: impl Fn(&App) -> bool) {
    for _ in 0..100 {
        let _ = app.poll_worker();
        if predicate(app) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("application state did not update");
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
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
