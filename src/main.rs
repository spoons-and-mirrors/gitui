mod app;
mod git;
mod ui;

use std::{io, path::PathBuf, time::Duration};

use anyhow::Result;
use app::App;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
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

    while !app.should_quit {
        app.poll_worker();
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.is_press() => app.handle_key(key),
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            Event::Paste(text) => app.handle_paste(&text),
            _ => {}
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
            EnableBracketedPaste
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
        DisableBracketedPaste
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
