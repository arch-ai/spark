use std::io::{self, Stdout};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// A wrapper around ratatui's Terminal that handles setup/teardown
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    /// Create and initialize a new terminal
    pub fn new() -> io::Result<Self> {
        let ui_thread = std::thread::current().id();
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().id() == ui_thread {
                let _ = terminal::disable_raw_mode();
                let _ = execute!(
                    io::stdout(),
                    LeaveAlternateScreen,
                    Show,
                    DisableMouseCapture
                );
            }
            previous_hook(info);
        }));
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        let mut tui = Self { terminal };
        tui.enter()?;
        Ok(tui)
    }

    /// Enter the TUI mode (raw mode, alternate screen, etc.)
    fn enter(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            Hide,
            EnableMouseCapture
        )?;
        self.terminal.clear()?;
        Ok(())
    }

    /// Exit the TUI mode and restore terminal state
    fn exit(&mut self) -> io::Result<()> {
        let screen_result = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            Show,
            DisableMouseCapture
        );
        let raw_result = terminal::disable_raw_mode();
        screen_result.and(raw_result)
    }

    /// Get a mutable reference to the terminal for rendering
    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    // draw() and size() removed; terminal is accessed directly by the runner
}

impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(e) = self.exit() {
            eprintln!("Error restoring terminal: {e}");
        }
    }
}
