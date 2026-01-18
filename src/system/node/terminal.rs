use std::io;
use std::process::Command;

pub fn open_path_location(path: &std::path::Path) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
    } else {
        Command::new("xdg-open")
    };

    command.arg(path);
    command.spawn().map(|_| ())
}
