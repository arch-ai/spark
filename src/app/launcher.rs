//! Stop the opposite configured mode successfully before launching a mode.
use super::workspace_config::{self, ProjectConfig};
use crate::system::live::{LiveStream, StreamCancel, StreamMessage};
use std::io;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dev,
    Prod,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "prod",
        }
    }
}
pub enum LaunchMessage {
    Line(String, bool),
    Done(Result<String, String>),
}
#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    current: Mutex<Option<StreamCancel>>,
}
pub struct LaunchJob {
    pub rx: mpsc::Receiver<LaunchMessage>,
    control: Arc<Control>,
}
impl LaunchJob {
    pub fn start(config: ProjectConfig, mode: Mode) -> Self {
        let (tx, rx) = mpsc::sync_channel(256);
        let control = Arc::new(Control::default());
        let work_control = control.clone();
        std::thread::spawn(move || {
            let result = run(&config, mode, &work_control, |text, stderr| {
                let _ = tx.try_send(LaunchMessage::Line(text, stderr));
            });
            let _ = tx.send(LaunchMessage::Done(result.map_err(|e| e.to_string())));
        });
        Self { rx, control }
    }
    pub fn cancel(&self) {
        self.control.cancelled.store(true, Ordering::SeqCst);
        if let Some(current) = self
            .control
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            current.cancel();
        }
    }
}
impl Drop for LaunchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn run(
    config: &ProjectConfig,
    mode: Mode,
    control: &Control,
    mut line: impl FnMut(String, bool),
) -> io::Result<String> {
    let (stop, start) = match mode {
        Mode::Dev => (&config.stop_prod, &config.start_dev),
        Mode::Prod => (&config.stop_dev, &config.start_prod),
    };
    // Validate both prerequisites before the first side effect.
    let stop = workspace_config::script_path(config, stop)?;
    let start = workspace_config::script_path(config, start)?;
    for (phase, path) in [
        ("Stopping opposite mode", stop),
        ("Starting selected mode", start),
    ] {
        let mut current = control.current.lock().unwrap_or_else(|e| e.into_inner());
        if control.cancelled.load(Ordering::SeqCst) {
            return Err(io::Error::other("Launch canceled"));
        }
        line(format!("{phase}: {}", path.display()), false);
        let stream = LiveStream::spawn(Command::new(&path).current_dir(&config.path))?;
        *current = Some(stream.cancel_token());
        drop(current);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match if phase.starts_with("Stopping") {
                stream
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Opposite stop script exceeded 120 seconds; start script was not run",
                        )
                    })
            } else {
                stream.recv().map_err(io::Error::other)
            }? {
                StreamMessage::Line { text, stderr } => line(text, stderr),
                StreamMessage::Exit(result) => {
                    *control.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    let status = result?;
                    if !status.success() {
                        return Err(io::Error::other(format!(
                            "{phase} failed ({status}); {}",
                            if phase.starts_with("Stopping") {
                                "start script was not run"
                            } else {
                                "check script output"
                            }
                        )));
                    }
                    break;
                }
            }
        }
    }
    Ok(format!(
        "{} scripts completed successfully. Service state is shown by the resource snapshots.",
        mode.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn config() -> (std::path::PathBuf, ProjectConfig) {
        let root = std::env::temp_dir().join(format!(
            "spark-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let config = ProjectConfig {
            name: "demo".into(),
            path: root.to_string_lossy().into(),
            start_dev: "start-dev".into(),
            stop_dev: "stop-dev".into(),
            start_prod: "start-prod".into(),
            stop_prod: "stop-prod".into(),
        };
        for script in ["start-dev", "stop-dev", "start-prod", "stop-prod"] {
            let path = root.join(script);
            std::fs::write(
                &path,
                format!("#!/bin/sh\nprintf '{script}\\n' >> order\nprintf '{script}\\n'\n"),
            )
            .unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        (root, config)
    }
    #[test]
    fn modes_stop_the_other_before_starting_and_do_not_guess_scripts() {
        let (root, config) = config();
        run(&config, Mode::Dev, &Control::default(), |_, _| {}).unwrap();
        run(&config, Mode::Prod, &Control::default(), |_, _| {}).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("order")).unwrap(),
            "stop-prod\nstart-dev\nstop-dev\nstart-prod\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_failed_stop_blocks_start_and_retains_output() {
        let (root, config) = config();
        std::fs::write(
            root.join("stop-prod"),
            "#!/bin/sh\nprintf 'stop failed\\n' >&2\nexit 9\n",
        )
        .unwrap();
        let mut output = Vec::new();
        let error = run(&config, Mode::Dev, &Control::default(), |text, _| {
            output.push(text)
        })
        .unwrap_err();
        assert!(error.to_string().contains("start script was not run"));
        assert!(output.iter().any(|line| line == "stop failed"));
        assert!(!root.join("order").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
