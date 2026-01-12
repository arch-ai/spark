mod app;
mod system;
mod ui;
mod util;

use std::io;

fn main() -> io::Result<()> {
    // Tui handles setup/teardown via Drop
    let mut tui = app::Tui::new()?;

    let result = app::run_ratatui(tui.terminal());

    if let Err(err) = result {
        eprintln!("Error: {err}");
    }

    Ok(())
}
