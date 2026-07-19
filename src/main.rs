mod app;
mod git;
mod repository_session;
mod selection;
mod theme;
mod tree;
mod ui;

use std::{io, path::PathBuf, time::Duration};

use anyhow::Result;
use app::{App, Mode};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    install_panic_hook();
    let mut terminal = start_terminal()?;
    let _guard = TerminalGuard;
    let mut app = App::new(path);
    let mut dirty = true;

    while !app.should_quit {
        dirty |= app.poll_worker();
        if dirty {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        for _ in 0..64 {
            let (changed, render_before_next_event) = match event::read()? {
                Event::Key(key) if key.is_press() => {
                    app.handle_key(key);
                    (true, false)
                }
                Event::Mouse(mouse) => {
                    let changed = !matches!(mouse.kind, event::MouseEventKind::Moved)
                        || app.mode == Mode::ActionMenu;
                    app.handle_mouse(mouse);
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
    }

    Ok(())
}

fn start_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let result = (|| {
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        terminal.clear()?;
        Ok(terminal)
    })();
    if result.is_err() {
        restore_terminal();
    }
    result
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        PopKeyboardEnhancementFlags
    );
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
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
