use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn open_container_shell(container_id: &str) -> io::Result<()> {
    let cmd = format!(
        "docker exec -it {id} bash 2>/dev/null || docker exec -it {id} sh; exec bash",
        id = container_id
    );
    open_terminal(&cmd)
}

pub fn open_container_logs(container_id: &str) -> io::Result<()> {
    let cmd = format!(
        "docker logs -f --tail 200 {id}; exec bash",
        id = container_id
    );
    open_terminal(&cmd)
}

fn open_terminal(cmd: &str) -> io::Result<()> {
    let mut last_err = None;
    if let Ok(term) = env::var("TERMINAL") {
        match try_spawn_terminal(&term, terminal_mode(&term), cmd) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => last_err = Some(err),
        }
    }

    let candidates = [
        "terminator",
        "gnome-terminal",
        "x-terminal-emulator",
        "konsole",
        "xfce4-terminal",
        "mate-terminal",
        "tilix",
        "xterm",
    ];

    for name in candidates {
        match try_spawn_terminal(name, terminal_mode(name), cmd) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "No supported terminal found; install a terminal or set TERMINAL to its executable",
        )
    }))
}

fn terminal_mode(term: &str) -> TerminalMode {
    match Path::new(term).file_name().and_then(|name| name.to_str()) {
        Some("terminator" | "xfce4-terminal") => TerminalMode::DashX,
        Some("gnome-terminal" | "mate-terminal") => TerminalMode::DoubleDash,
        _ => TerminalMode::DashE,
    }
}

enum TerminalMode {
    DashE,
    DashX,
    DoubleDash,
}

fn try_spawn_terminal(term: &str, mode: TerminalMode, cmd: &str) -> io::Result<()> {
    let mut command = Command::new(term);
    match mode {
        TerminalMode::DashE => {
            command.args(["-e", "bash", "-lc", cmd]);
        }
        TerminalMode::DashX => {
            command.args(["-x", "bash", "-lc", cmd]);
        }
        TerminalMode::DoubleDash => {
            command.args(["--", "bash", "-lc", cmd]);
        }
    }
    // Launcher diagnostics must not write over Spark's terminal display.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
