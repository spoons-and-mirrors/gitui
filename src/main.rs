mod app;
mod diagnostics;
mod filesystem;
mod formatter;
mod git;
mod process;
mod repository_session;
mod selection;
mod theme;
mod tree;
mod ui;

use std::{io, path::PathBuf, process::Command, time::Duration};

use anyhow::Result;
use app::{App, EditorRequest};
use crossterm::{
    cursor::MoveTo,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    if let Ok(log_path) = diagnostics::init() {
        diagnostics::event(format!(
            "startup pid={} path={} log={}",
            std::process::id(),
            path.display(),
            log_path.display()
        ));
    }
    install_panic_hook();
    let mut terminal = start_terminal()?;
    let _guard = TerminalGuard;
    let mut app = App::opening(path);
    let mut dirty = true;

    while !app.should_quit {
        dirty |= {
            let _activity = diagnostics::activity("poll-workers", app.diagnostic_context());
            app.poll_worker()
        };
        if dirty {
            let _activity = diagnostics::activity("draw", app.diagnostic_context());
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            dirty = false;
        }
        let ready = {
            let _activity = diagnostics::activity("terminal-poll", app.diagnostic_context());
            event::poll(Duration::from_millis(50))?
        };
        if !ready {
            continue;
        }
        for _ in 0..64 {
            let _activity = diagnostics::activity("input", app.diagnostic_context());
            let (changed, render_before_next_event) = match event::read()? {
                Event::Key(key) if key.is_press() => {
                    app.handle_key(key);
                    (true, false)
                }
                Event::Mouse(mouse) => {
                    let hover_before = (
                        app.changes.hunk_selection,
                        app.actions.selection,
                        app.graph_state.selected(),
                        app.author_filter.state.selected(),
                        app.repository_browser.state.selected(),
                        app.workspace_panel.selected,
                        app.hovered_hit_target,
                    );
                    app.handle_mouse(mouse);
                    let changed = !matches!(mouse.kind, event::MouseEventKind::Moved)
                        || hover_before
                            != (
                                app.changes.hunk_selection,
                                app.actions.selection,
                                app.graph_state.selected(),
                                app.author_filter.state.selected(),
                                app.repository_browser.state.selected(),
                                app.workspace_panel.selected,
                                app.hovered_hit_target,
                            );
                    (changed, false)
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                    (true, false)
                }
                Event::Resize(_, _) => (true, true),
                _ => (false, false),
            };
            dirty |= changed;
            if render_before_next_event
                || app.requires_render_before_next_event()
                || !event::poll(Duration::ZERO)?
            {
                break;
            }
        }
        if let Some(text) = app.take_copy_request() {
            app.notice = Some(match selection::copy_to_clipboard(&text) {
                Ok(()) => "Copied selection".to_owned(),
                Err(error) => format!("Could not copy selection: {error}"),
            });
            dirty = true;
        }
        if let Some(request) = app.take_editor_request() {
            restore_terminal();
            let result = run_editor(request);
            terminal = start_terminal()?;
            app.editor_finished(result);
            dirty = true;
        }
    }

    app.flush_commit_draft();
    diagnostics::event("shutdown clean".to_owned());

    Ok(())
}

fn run_editor(request: EditorRequest) -> Result<(), String> {
    let Some((program, args)) = request.command.split_first() else {
        return Err("Editor command is empty".to_owned());
    };
    let status = Command::new(program)
        .args(args)
        .arg(&request.file)
        .current_dir(&request.repository)
        .status()
        .map_err(|error| format!("Could not start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Editor exited with status {}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ))
    }
}

fn start_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let result = (|| -> Result<_> {
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            Clear(ClearType::All),
            MoveTo(0, 0)
        )?;
        Ok(Terminal::new(CrosstermBackend::new(stdout))?)
    })();
    if result.is_err() {
        restore_terminal();
    }
    result
}

fn restore_terminal() {
    // Keyboard enhancement was pushed inside the alternate screen, so unwind it first.
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        diagnostics::panic(info.to_string());
        restore_terminal();
        original(info);
    }));
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}
