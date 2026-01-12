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
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            Show,
            DisableMouseCapture
        )?;
        terminal::disable_raw_mode()?;
        Ok(())
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
