mod app;
mod system;
mod ui;
mod util;

use std::io;

fn main() -> io::Result<()> {
    // Tui handles setup/teardown via Drop
    let mut tui = app::Tui::new()?;

    let result = app::run_ratatui(tui.terminal());

    // Restore the terminal before main prints an error and returns a failure status.
    drop(tui);
    result
}
