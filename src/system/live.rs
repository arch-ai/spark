//! Owned, cancellable pipe streams. Readers block; no subprocess polling.
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
mod lifetime;

const QUEUE_LIMIT: usize = 256;
const LINE_LIMIT: usize = 16 * 1024;

pub enum StreamMessage {
    Line { text: String, stderr: bool },
    Exit(io::Result<ExitStatus>),
}

struct Group {
    pid: i32,
    active: bool,
    wake: Option<Arc<std::os::fd::OwnedFd>>,
}

#[derive(Clone)]
pub struct StreamCancel {
    group: Arc<Mutex<Group>>,
}
impl StreamCancel {
    pub fn cancel(&self) {
        let guard = self.group.lock().unwrap_or_else(|e| e.into_inner());
        if guard.active {
            unsafe {
                libc::kill(-guard.pid, libc::SIGKILL);
            }
        }
        if let Some(wake) = &guard.wake {
            lifetime::wake(wake);
        }
    }
}

pub struct LiveStream {
    rx: mpsc::Receiver<StreamMessage>,
    group: Arc<Mutex<Group>>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}

impl LiveStream {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let mut child = command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let group = Arc::new(Mutex::new(Group {
            pid: child.id() as i32,
            active: true,
            wake: None,
        }));
        let queued = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let readers = [
            (Box::new(stdout) as Box<dyn Read + Send>, false),
            (Box::new(stderr) as Box<dyn Read + Send>, true),
        ]
        .map(|(reader, stderr)| {
            let tx = tx.clone();
            let queued = queued.clone();
            let dropped = dropped.clone();
            thread::spawn(move || read_lines(reader, stderr, tx, queued, dropped))
        });
        let wait_group = group.clone();
        thread::spawn(move || {
            // Keep the leader unreaped until both readers stop. The group ID cannot
            // be reused while cancellation still has permission to signal it.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        child.id(),
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if result == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
            for reader in readers {
                let _ = reader.join();
            }
            let mut guard = wait_group.lock().unwrap_or_else(|e| e.into_inner());
            guard.active = false;
            let result = child.wait();
            drop(guard);
            let _ = tx.send(StreamMessage::Exit(result));
        });
        Ok(Self {
            rx,
            group,
            queued,
            dropped,
        })
    }

    pub fn drain(&self) -> Vec<StreamMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = self.rx.try_recv() {
            if matches!(message, StreamMessage::Line { .. }) {
                self.queued.fetch_sub(1, Ordering::Relaxed);
            }
            messages.push(message);
        }
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped != 0 {
            messages.push(StreamMessage::Line {
                text: format!("[Spark: {dropped} lines dropped; stream buffer full]"),
                stderr: true,
            });
        }
        messages
    }

    pub fn cancel(&self) {
        self.cancel_token().cancel();
    }
    pub fn cancel_token(&self) -> StreamCancel {
        StreamCancel {
            group: self.group.clone(),
        }
    }
    fn stop_on_process_exit(&self, pid: u32, identity: u64) -> io::Result<()> {
        let wake = lifetime::watch(pid, identity, self.cancel_token())?;
        self.group.lock().unwrap_or_else(|e| e.into_inner()).wake = Some(wake);
        Ok(())
    }

    /// Blocking consumer for sequential configured scripts.
    pub fn recv(&self) -> Result<StreamMessage, mpsc::RecvError> {
        let message = self.rx.recv()?;
        if matches!(message, StreamMessage::Line { .. }) {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(message)
    }
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<StreamMessage, mpsc::RecvTimeoutError> {
        let message = self.rx.recv_timeout(timeout)?;
        if matches!(message, StreamMessage::Line { .. }) {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(message)
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn read_lines(
    mut reader: Box<dyn Read + Send>,
    stderr: bool,
    tx: mpsc::Sender<StreamMessage>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
) {
    let mut chunk = [0; 8192];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let count = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                emit(
                    format!("Stream read failed: {error}"),
                    true,
                    &tx,
                    &queued,
                    &dropped,
                );
                break;
            }
        };
        for &byte in &chunk[..count] {
            if byte == b'\n' {
                let mut text = String::from_utf8_lossy(&line)
                    .trim_end_matches('\r')
                    .to_string();
                if truncated {
                    text.push_str(" [line truncated]");
                }
                emit(text, stderr, &tx, &queued, &dropped);
                line.clear();
                truncated = false;
            } else if line.len() < LINE_LIMIT {
                line.push(byte);
            } else {
                truncated = true;
            }
        }
    }
    if !line.is_empty() {
        let mut text = String::from_utf8_lossy(&line).into_owned();
        if truncated {
            text.push_str(" [line truncated]");
        }
        emit(text, stderr, &tx, &queued, &dropped);
    }
}

fn emit(
    text: String,
    stderr: bool,
    tx: &mpsc::Sender<StreamMessage>,
    queued: &AtomicUsize,
    dropped: &AtomicUsize,
) {
    if queued
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < QUEUE_LIMIT).then_some(n + 1)
        })
        .is_ok()
    {
        if tx
            .send(StreamMessage::Line {
                text: plain_text(&text),
                stderr,
            })
            .is_err()
        {
            queued.fetch_sub(1, Ordering::Relaxed);
        }
    } else {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn plain_text(text: &str) -> String {
    let mut chars = text.chars();
    let mut result = String::with_capacity(text.len());
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut escape = false;
                    for ch in chars.by_ref() {
                        if ch == '\x07' || escape && ch == '\\' {
                            break;
                        }
                        escape = ch == '\x1b';
                    }
                }
                _ => {}
            }
        } else if ch == '\t' {
            result.push_str("  ");
        } else if !ch.is_control() {
            result.push(ch);
        }
    }
    result
}

pub fn container_logs(id: &str) -> io::Result<LiveStream> {
    LiveStream::spawn(Command::new("docker").args([
        "logs",
        "--follow",
        "--timestamps",
        "--tail",
        "200",
        id,
    ]))
}
pub fn docker_events() -> io::Result<LiveStream> {
    LiveStream::spawn(Command::new("docker").args([
        "events",
        "--format",
        "{{json .}}",
        "--since",
        "0s",
    ]))
}
pub fn pm2_logs(id: u32) -> io::Result<LiveStream> {
    let id = id.to_string();
    match LiveStream::spawn(Command::new("pm2").args(["logs", &id, "--lines", "200", "--raw"])) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            LiveStream::spawn(Command::new("bash").args([
                "-lc",
                "exec pm2 \"$@\"",
                "spark-pm2",
                "logs",
                &id,
                "--lines",
                "200",
                "--raw",
            ]))
        }
        result => result,
    }
}
pub fn process_logs(pid: u32, identity: u64) -> io::Result<LiveStream> {
    if super::process::process_identity(pid)? != identity {
        return Err(io::Error::other(
            "Process identity changed; reselect its owner",
        ));
    }
    // Following the journal never drains a process's stdout/stderr pipe.
    let boot = std::fs::read_to_string("/proc/stat")?
        .lines()
        .find_map(|line| {
            line.strip_prefix("btime ")
                .and_then(|value| value.parse::<u64>().ok())
        })
        .ok_or_else(|| io::Error::other("Kernel boot time unavailable"))?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return Err(io::Error::other("Kernel clock frequency unavailable"));
    }
    let since = format!("@{}", boot.saturating_add(identity / hz as u64));
    let stream = LiveStream::spawn(Command::new("journalctl").args([
        "--follow",
        "--no-pager",
        "-n",
        "200",
        "--boot",
        "--since",
        &since,
        &format!("_PID={pid}"),
    ]))?;
    stream.stop_on_process_exit(pid, identity)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_cannot_inject_terminal_controls_or_clipboard_commands() {
        assert_eq!(
            plain_text("\x1b[31mERROR\x1b[0m \x1b]52;c;c2VjcmV0\x07 café\tready"),
            "ERROR  café  ready"
        );
    }
    #[test]
    fn streams_both_pipes_and_exit_without_polling() {
        let stream = LiveStream::spawn(
            Command::new("sh").args(["-c", "printf 'hello\\n'; printf 'failure\\n' >&2; exit 7"]),
        )
        .unwrap();
        let mut lines = Vec::new();
        loop {
            match stream.recv().unwrap() {
                StreamMessage::Line { text, stderr } => lines.push((text, stderr)),
                StreamMessage::Exit(result) => {
                    assert_eq!(result.unwrap().code(), Some(7));
                    break;
                }
            }
        }
        assert!(lines.contains(&("hello".into(), false)));
        assert!(lines.contains(&("failure".into(), true)));
    }
    #[test]
    fn cancellation_terminates_owned_group_and_reaps_leader() {
        let stream =
            LiveStream::spawn(Command::new("sh").args(["-c", "printf 'ready\\n'; exec sleep 30"]))
                .unwrap();
        let pid = stream.group.lock().unwrap().pid;
        assert!(matches!(stream.recv().unwrap(), StreamMessage::Line { .. }));
        stream.cancel();
        assert!(matches!(stream.recv().unwrap(), StreamMessage::Exit(Ok(_))));
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
    #[test]
    fn huge_lines_are_bounded() {
        let stream = LiveStream::spawn(
            Command::new("sh").args(["-c", "head -c 100000 /dev/zero; printf '\\n'"]),
        )
        .unwrap();
        match stream.recv().unwrap() {
            StreamMessage::Line { text, .. } => assert!(text.len() <= LINE_LIMIT + 30),
            _ => panic!("missing line"),
        }
    }
    #[test]
    fn native_exit_closes_its_log_reader_through_kernel_notification() {
        let mut native = Command::new("sleep").arg("30").spawn().unwrap();
        let identity = super::super::process::process_identity(native.id()).unwrap();
        let stream =
            LiveStream::spawn(Command::new("sh").args(["-c", "printf 'ready\\n'; exec sleep 30"]))
                .unwrap();
        stream.stop_on_process_exit(native.id(), identity).unwrap();
        assert!(matches!(stream.recv().unwrap(), StreamMessage::Line { .. }));
        native.kill().unwrap();
        native.wait().unwrap();
        assert!(matches!(
            stream
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            StreamMessage::Exit(Ok(_))
        ));
    }
}
