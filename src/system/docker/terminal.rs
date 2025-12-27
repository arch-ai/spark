use std::env;
use std::io;
use std::process::Command;

pub fn open_container_shell(container_id: &str) -> io::Result<()> {
    let cmd = format!(
        "docker exec -it {id} bash 2>/dev/null || docker exec -it {id} sh; exec bash",
        id = container_id
    );
    if let Ok(term) = env::var("TERMINAL") {
        if try_spawn_terminal(&term, TerminalMode::DashE, &cmd).is_ok() {
            return Ok(());
        }
    }

    let mut last_err = None;
    let candidates = [
        ("x-terminal-emulator", TerminalMode::DashE),
        ("gnome-terminal", TerminalMode::DoubleDash),
        ("konsole", TerminalMode::DashE),
        ("xfce4-terminal", TerminalMode::DashE),
        ("mate-terminal", TerminalMode::DoubleDash),
        ("tilix", TerminalMode::DashE),
        ("xterm", TerminalMode::DashE),
    ];

    for (name, mode) in candidates {
        match try_spawn_terminal(name, mode, &cmd) {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "No supported terminal found")
    }))
}

pub fn open_container_logs(container_id: &str) -> io::Result<()> {
    let cmd = format!("docker logs -f --tail 200 {id}; exec bash", id = container_id);
    if let Ok(term) = env::var("TERMINAL") {
        if try_spawn_terminal(&term, TerminalMode::DashE, &cmd).is_ok() {
            return Ok(());
        }
    }

    let mut last_err = None;
    let candidates = [
        ("x-terminal-emulator", TerminalMode::DashE),
        ("gnome-terminal", TerminalMode::DoubleDash),
        ("konsole", TerminalMode::DashE),
        ("xfce4-terminal", TerminalMode::DashE),
        ("mate-terminal", TerminalMode::DoubleDash),
        ("tilix", TerminalMode::DashE),
        ("xterm", TerminalMode::DashE),
    ];

    for (name, mode) in candidates {
        match try_spawn_terminal(name, mode, &cmd) {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "No supported terminal found")
    }))
}

enum TerminalMode {
    DashE,
    DoubleDash,
}

fn try_spawn_terminal(term: &str, mode: TerminalMode, cmd: &str) -> io::Result<()> {
    let mut command = Command::new(term);
    match mode {
        TerminalMode::DashE => {
            command.args(["-e", "bash", "-lc", cmd]);
        }
        TerminalMode::DoubleDash => {
            command.args(["--", "bash", "-lc", cmd]);
        }
    }
    command.spawn().map(|_| ())
}
