//! Docker command deadlines. Interactive shells use terminal.rs.
use super::super::command::run_output;
use std::io;
use std::process::{Command, Output};
use std::time::Duration;

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
        run_output(self, timeout, "Docker")
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
}
