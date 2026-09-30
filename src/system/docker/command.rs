//! Bounded, non-interactive Docker CLI execution. Interactive shells use terminal.rs.
use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const VOLUME_REMOVE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(crate) trait DockerCommand {
    fn docker_output(&mut self) -> io::Result<Output>;
}

impl DockerCommand for Command {
    fn docker_output(&mut self) -> io::Result<Output> {
        let mut args = self.get_args();
        let operation = args.next().and_then(|arg| arg.to_str());
        let subcommand = args.next().and_then(|arg| arg.to_str());
        let timeout = command_timeout(operation, subcommand);
        run_output(self, timeout)
    }
}

fn command_timeout(operation: Option<&str>, subcommand: Option<&str>) -> Duration {
    if operation == Some("volume") && matches!(subcommand, Some("rm" | "prune")) {
        return VOLUME_REMOVE_TIMEOUT;
    }
    let mutating = matches!(
        operation,
        Some("start" | "stop" | "restart" | "kill" | "rm")
    ) || matches!(
        (operation, subcommand),
        (Some("image" | "volume" | "builder"), Some("prune" | "rm"))
    );
    Duration::from_secs(if mutating { 120 } else { 15 })
}

fn read_bounded(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    let mut overflow = false;
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
        overflow |= keep != count;
    }
    if overflow {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Docker output exceeded 8 MiB; narrow the request",
        ))
    } else {
        Ok(output)
    }
}

fn run_output(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    thread::spawn(move || {
        let _ = out_tx.send((true, read_bounded(stdout)));
    });
    thread::spawn(move || {
        let _ = tx.send((false, read_bounded(stderr)));
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let mut stdout = None;
        let mut stderr = None;
        // Both pipes are drained concurrently, even when either exceeds the limit.
        for _ in 0..2 {
            let (is_stdout, bytes) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Docker command timed out"))?;
            if is_stdout {
                stdout = Some(bytes);
            } else {
                stderr = Some(bytes);
            }
        }
        let stdout = stdout.unwrap()?;
        let stderr = stderr.unwrap()?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Docker command timed out",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        };
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })();
    if result.is_err() {
        #[cfg(unix)]
        // SAFETY: the child was spawned in its own process group, owned by this call.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result.map_err(|err: io::Error| {
        if err.kind() == io::ErrorKind::TimedOut {
            io::Error::new(err.kind(), format!("Stopped waiting for Docker after {}s. Docker may still be completing the operation. Refresh and check its state before retrying.", timeout.as_secs()))
        } else { err }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_volume_cleanup_has_a_separate_deadline() {
        for action in ["rm", "prune"] {
            assert_eq!(
                command_timeout(Some("volume"), Some(action)),
                Duration::from_secs(1800)
            );
        }
        assert_eq!(
            command_timeout(Some("volume"), Some("inspect")),
            Duration::from_secs(15)
        );
        assert_eq!(
            command_timeout(Some("stop"), Some("container")),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn drains_both_pipes_and_preserves_exit_status() {
        let output = run_output(
            Command::new("sh").args([
                "-c",
                "head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2; exit 7",
            ]),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 100000);
        assert_eq!(output.stderr.len(), 100000);
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn times_out_and_reaps_a_hung_command() {
        let started = Instant::now();
        let err = run_output(
            Command::new("sh").args(["-c", "sleep 30"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn limits_output_without_deadlocking() {
        let err = run_output(
            Command::new("head").args(["-c", "9000000", "/dev/zero"]),
            Duration::from_secs(3),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
