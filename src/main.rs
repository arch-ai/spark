mod app;
mod system;
mod ui;
mod util;

use std::io;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::style::ResetColor;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture)?;

    let result = app::run(&mut stdout);

    execute!(stdout, LeaveAlternateScreen, Show, ResetColor, DisableMouseCapture)?;
    terminal::disable_raw_mode()?;

    if let Err(err) = result {
        eprintln!("Error: {err}");
    }

    Ok(())
}
