//! Bounded execution shared by non-interactive CLI integrations.
use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

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
            "Command output exceeded 8 MiB; narrow the request",
        ))
    } else {
        Ok(output)
    }
}

pub(crate) fn run_output(
    command: &mut Command,
    timeout: Duration,
    label: &str,
) -> io::Result<Output> {
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
    let (exit_tx, exit_rx) = mpsc::channel();
    // Block in wait on a dedicated thread; command completion needs no polling.
    thread::spawn(move || {
        let _ = exit_tx.send(child.wait());
    });
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
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Command timed out"))?;
            if is_stdout {
                stdout = Some(bytes);
            } else {
                stderr = Some(bytes);
            }
        }
        let stdout = stdout.unwrap()?;
        let stderr = stderr.unwrap()?;
        let status = exit_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Command timed out"))??;
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
        // The waiter owns/reaps the child, even if kernel I/O delays its exit.
        let _ = exit_rx.recv_timeout(Duration::from_millis(250));
    }
    result.map_err(|err: io::Error| {
        if err.kind() == io::ErrorKind::TimedOut {
            io::Error::new(err.kind(), format!("Stopped waiting for {label} after {}s. The operation may still be completing. Refresh and check its state before retrying.", timeout.as_secs()))
        } else { err }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_both_pipes_and_preserves_exit_status() {
        let output = run_output(
            Command::new("sh").args([
                "-c",
                "head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2; exit 7",
            ]),
            Duration::from_secs(3),
            "test",
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 100000);
        assert_eq!(output.stderr.len(), 100000);
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn times_out_and_reaps_a_hung_command() {
        let started = Instant::now();
        let pid_file = std::env::temp_dir().join(format!(
            "spark-command-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let err = run_output(
            Command::new("sh")
                .args([
                    "-c",
                    "printf '%s' \"$$\" > \"$1\"; exec sleep 30",
                    "spark-test",
                ])
                .arg(&pid_file),
            Duration::from_millis(100),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        std::fs::remove_file(pid_file).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "timed-out child must be reaped"
        );
    }

    #[test]
    fn timeout_still_applies_after_a_command_closes_both_output_pipes() {
        let started = Instant::now();
        let err = run_output(
            Command::new("sh").args(["-c", "exec 1>&- 2>&-; exec sleep 30"]),
            Duration::from_millis(100),
            "test",
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
            "test",
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
