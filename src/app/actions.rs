use sysinfo::{Pid, System};

use crate::app::{AppState, InputMode, ViewMode};
use crate::app::state::{LogSource, OperationComplete};
use crate::system::{docker, node, process};

/// Check if a PID is managed by PM2 and return the PM2 ID if found
fn find_pm2_id_for_pid(pid: u32) -> Option<u32> {
    if !node::is_pm2_running() {
        return None;
    }

    let pm2_procs = node::load_pm2_processes().ok()?;
    pm2_procs
        .iter()
        .find(|p| p.pid == Some(pid))
        .map(|p| p.pm_id)
}

/// Check if a process is managed by nodemon/tsx/ts-node-dev and return the parent PID
fn find_supervisor_parent(pid: Pid, system: &System) -> Option<(Pid, String)> {
    let process = system.process(pid)?;
    let parent_pid = process.parent()?;
    let parent = system.process(parent_pid)?;
    let parent_name = parent.name().to_lowercase();

    // Check for common Node.js development supervisors
    if parent_name.contains("nodemon")
        || parent_name.contains("tsx")
        || parent_name.contains("ts-node-dev")
        || parent_name.contains("node-dev") {
        Some((parent_pid, parent.name().to_string()))
    } else {
        None
    }
}

pub(crate) fn kill_selected_process(state: &mut AppState, system: &mut System) {
    let Some(pid) = state.visible_pids.get(state.selected).cloned() else {
        state.set_message("No process selected");
        return;
    };

    match system.process(pid) {
        Some(process) => {
            let name = process.name().to_string();
            if process.kill() {
                state.set_message(format!("Killed PID {} ({})", pid, name));
            } else {
                state.set_message(format!("Failed to kill PID {} ({})", pid, name));
            }
        }
        None => {
            state.set_message(format!("Process PID {} not found", pid));
        }
    }
}

pub(crate) fn kill_selected_port_process(state: &mut AppState, system: &mut System) {
    use sysinfo::Signal;

    let Some(pid) = state.visible_ports.get(state.selected).cloned() else {
        state.set_message("No port selected");
        return;
    };
    if pid == Pid::from_u32(0) {
        let container_id = state
            .visible_ports_container_ids
            .get(state.selected)
            .and_then(|id| id.clone());
        if let Some(id) = container_id {
            match docker::kill_container(&id) {
                Ok(()) => {
                    state.set_message(format!("Killed container {}", id));
                }
                Err(err) => {
                    state.set_message(format!("Failed to kill container: {err}"));
                }
            }
        } else {
            state.set_message("No process associated with this port");
        }
        return;
    }

    let pid_u32 = pid.as_u32();

    // Check if this process is managed by PM2
    if let Some(pm2_id) = find_pm2_id_for_pid(pid_u32) {
        match node::pm2_stop(pm2_id) {
            Ok(()) => {
                state.set_message(format!("Stopped PM2 process {} (PID {})", pm2_id, pid_u32));
            }
            Err(err) => {
                state.set_message(format!("Failed to stop PM2 process: {}", err));
            }
        }
        return;
    }

    // Check if this process is managed by nodemon/tsx/ts-node-dev
    if let Some((supervisor_pid, supervisor_name)) = find_supervisor_parent(pid, system) {
        if let Some(supervisor) = system.process(supervisor_pid) {
            // Kill the supervisor parent instead of the child
            let mut killed = supervisor.kill_with(Signal::Term).unwrap_or(false);
            if !killed {
                state.set_message(format!("Failed to signal {} (PID {})", supervisor_name, supervisor_pid));
                return;
            }

            // Wait briefly for graceful shutdown
            std::thread::sleep(std::time::Duration::from_millis(200));
            system.refresh_processes();

            // If still running, send SIGKILL
            if system.process(supervisor_pid).is_some() {
                if let Some(process) = system.process(supervisor_pid) {
                    killed = process.kill_with(Signal::Kill).unwrap_or(false);
                }
            }

            if killed {
                state.set_message(format!("Killed {} (PID {}) and child process (PID {})", supervisor_name, supervisor_pid, pid_u32));
            } else {
                state.set_message(format!("Failed to kill {} (PID {})", supervisor_name, supervisor_pid));
            }
            return;
        }
    }

    match system.process(pid) {
        Some(process) => {
            let name = process.name().to_string();
            // Try SIGTERM first
            let mut killed = process.kill_with(Signal::Term).unwrap_or(false);
            if !killed {
                state.set_message(format!("Failed to signal PID {} ({})", pid, name));
                return;
            }

            // Wait briefly for graceful shutdown
            std::thread::sleep(std::time::Duration::from_millis(200));
            system.refresh_processes();

            // If still running, send SIGKILL
            if system.process(pid).is_some() {
                if let Some(process) = system.process(pid) {
                    killed = process.kill_with(Signal::Kill).unwrap_or(false);
                }
            }

            if killed {
                state.set_message(format!("Killed PID {} ({})", pid, name));
            } else {
                state.set_message(format!("Failed to kill PID {} ({})", pid, name));
            }
        }
        None => {
            state.set_message(format!("Process PID {} not found", pid));
        }
    }
}

pub(crate) fn kill_selected_in_docker(state: &mut AppState) {
    use crate::system::docker::DockerRow;

    let Some(row) = state.docker_rows.get(state.docker_selected_row) else {
        state.set_message("No selection");
        return;
    };

    match row {
        DockerRow::Group { name, path, .. } => {
            // Kill all containers in this group
            let group_path = path.clone();
            let group_name = name.clone();
            let mut container_ids = Vec::new();

            for (i, container_group_path) in state.visible_container_group_path.iter().enumerate() {
                let matches = match &group_path {
                    Some(gp) => container_group_path == gp,
                    None => container_group_path == "-" || container_group_path.is_empty(),
                };
                if matches {
                    if let Some(id) = state.visible_containers.get(i) {
                        container_ids.push(id.clone());
                    }
                }
            }

            if container_ids.is_empty() {
                state.set_message("No containers in group");
                return;
            }

            let (success, failed) = docker::kill_containers(&container_ids);
            if failed == 0 {
                state.set_message(format!("Killed {} containers in {}", success, group_name));
            } else {
                state.set_message(format!(
                    "Killed {}/{} containers in {} ({} failed)",
                    success,
                    success + failed,
                    group_name,
                    failed
                ));
            }
        }
        DockerRow::Item { index, .. } => {
            // Kill single container
            let Some(container_id) = state.visible_containers.get(*index).cloned() else {
                state.set_message("No container selected");
                return;
            };
            let name = state
                .visible_container_names
                .get(*index)
                .cloned()
                .unwrap_or_else(|| container_id.clone());

            match docker::kill_container(&container_id) {
                Ok(()) => {
                    state.set_message(format!("Killed container {}", name));
                }
                Err(err) => {
                    state.set_message(format!("Failed to kill container: {err}"));
                }
            }
        }
        DockerRow::Separator => {
            state.set_message("Cannot kill separator");
        }
    }
}

pub(crate) fn open_selected_container(state: &mut AppState) {
    use crate::system::docker::DockerRow;

    let Some(row) = state.docker_rows.get(state.docker_selected_row) else {
        state.set_message("No container selected");
        return;
    };

    let container_index = match row {
        DockerRow::Item { index, .. } => *index,
        DockerRow::Group { .. } => {
            state.set_message("Select a container to open shell");
            return;
        }
        DockerRow::Separator => {
            state.set_message("No container selected");
            return;
        }
    };

    let Some(container_id) = state.visible_containers.get(container_index) else {
        state.set_message("No container selected");
        return;
    };

    match docker::open_container_shell(container_id) {
        Ok(()) => {
            state.set_message(format!("Opening shell in {container_id}"));
        }
        Err(err) => {
            state.set_message(format!("Failed to open terminal: {err}"));
        }
    }
}

pub(crate) fn open_selected_container_logs(state: &mut AppState) {
    use crate::system::docker::DockerRow;

    let Some(row) = state.docker_rows.get(state.docker_selected_row) else {
        state.set_message("No container selected");
        return;
    };

    let container_index = match row {
        DockerRow::Item { index, .. } => *index,
        DockerRow::Group { .. } => {
            state.set_message("Select a container to view logs");
            return;
        }
        DockerRow::Separator => {
            state.set_message("No container selected");
            return;
        }
    };

    let Some(container_id) = state.visible_containers.get(container_index).cloned() else {
        state.set_message("No container selected");
        return;
    };

    let title = format!("Docker logs: {}", container_id);
    let id = container_id.clone();
    start_log_fetch(
        state,
        title,
        LogSource::Docker {
            container_id: id.clone(),
        },
        move || docker::load_container_logs(&id),
    )
}

pub(crate) fn start_log_fetch<F>(
    state: &mut AppState,
    title: String,
    source: LogSource,
    command: F,
)
where
    F: FnOnce() -> std::io::Result<String> + Send + 'static,
{
    state.log_in_progress = Some(title.clone());
    state.log_output = None;
    state.log_output_hover = false;
    state.log_text.clear();
    state.log_lines.clear();
    state.log_wrap_width = 0;
    state.log_line_count = 0;
    state.log_scroll = 0;
    state.log_follow = true;
    state.log_last_scroll = std::time::Instant::now();
    state.log_source = Some(source);
    state.log_refresh_in_progress = true;
    state.log_last_refresh = std::time::Instant::now();
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = match result {
            Ok(output) => output,
            Err(err) => err.to_string(),
        };
        let _ = tx.send(OperationComplete {
            container_id: format!("logs::{}", title),
            success,
            message: if success { String::new() } else { output.clone() },
            output: Some(output),
        });
    });
}

pub(crate) fn start_log_refresh<F>(state: &mut AppState, title: String, command: F)
where
    F: FnOnce() -> std::io::Result<String> + Send + 'static,
{
    if state.log_refresh_in_progress {
        return;
    }
    state.log_refresh_in_progress = true;
    state.log_last_refresh = std::time::Instant::now();
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = match result {
            Ok(output) => output,
            Err(err) => err.to_string(),
        };
        let _ = tx.send(OperationComplete {
            container_id: format!("logs::{}", title),
            success,
            message: if success { String::new() } else { output.clone() },
            output: Some(output),
        });
    });
}

pub(crate) fn open_selected_env(state: &mut AppState, system: &System) {
    match state.view_mode {
        ViewMode::Docker => open_selected_container_env(state, ViewMode::Docker),
        ViewMode::Process => open_selected_process_env(state, system, ViewMode::Process),
        ViewMode::Ports => open_selected_ports_env(state, system),
        ViewMode::Node => open_selected_process_env(state, system, ViewMode::Node),
        ViewMode::DockerEnv => {}
    }
}

fn open_selected_container_env(state: &mut AppState, return_view: ViewMode) {
    use crate::system::docker::DockerRow;

    let Some(row) = state.docker_rows.get(state.docker_selected_row) else {
        state.set_message("No container selected");
        return;
    };

    let container_index = match row {
        DockerRow::Item { index, .. } => *index,
        DockerRow::Group { .. } => {
            state.set_message("Select a container to view env");
            return;
        }
        DockerRow::Separator => {
            state.set_message("No container selected");
            return;
        }
    };

    let Some(container_id) = state.visible_containers.get(container_index).cloned() else {
        state.set_message("No container selected");
        return;
    };
    let name = state
        .visible_container_names
        .get(container_index)
        .cloned()
        .unwrap_or_else(|| container_id.clone());
    let compose_name = state
        .visible_container_group_name
        .get(container_index)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "-".to_string());
    let compose_path = state
        .visible_container_group_path
        .get(container_index)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let port_public = state
        .visible_container_ports_public
        .get(container_index)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "-".to_string());
    let port_internal = state
        .visible_container_ports_internal
        .get(container_index)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "-".to_string());

    enter_env_view(
        state,
        return_view,
        "DOCKER ENV",
        format!("Compose: {compose_name}"),
        format!("Path: {compose_path}"),
        format!("Container: {name}"),
        format_ports_line(&port_public, &port_internal),
    );
    match docker::load_container_env(&container_id) {
        Ok(envs) => state.env_vars = envs,
        Err(err) => {
            state.env_vars = vec![format!("Failed to load env: {err}")];
        }
    }
}

fn open_selected_ports_env(state: &mut AppState, system: &System) {
    let Some(pid) = state.visible_ports.get(state.selected).cloned() else {
        state.set_message("No port selected");
        return;
    };
    if pid == Pid::from_u32(0) {
        let container_id = state
            .visible_ports_container_ids
            .get(state.selected)
            .and_then(|id| id.clone());
        if let Some(id) = container_id {
            enter_env_view(
                state,
                ViewMode::Ports,
                "CONTAINER ENV",
                format!("Container: {id}"),
                "Source: Ports".to_string(),
                "Compose: -".to_string(),
                "Ports: -".to_string(),
            );
            match docker::load_container_env(&id) {
                Ok(envs) => state.env_vars = envs,
                Err(err) => {
                    state.env_vars = vec![format!("Failed to load env: {err}")];
                }
            }
        } else {
            state.set_message("No process selected");
        }
        return;
    }

    open_process_env_for_pid(state, system, pid, ViewMode::Ports);
}

fn open_selected_process_env(state: &mut AppState, system: &System, return_view: ViewMode) {
    let Some(pid) = state.visible_pids.get(state.selected).copied() else {
        state.set_message("No process selected");
        return;
    };
    open_process_env_for_pid(state, system, pid, return_view);
}

fn open_process_env_for_pid(
    state: &mut AppState,
    system: &System,
    pid: Pid,
    return_view: ViewMode,
) {
    let Some(process) = system.process(pid) else {
        state.set_message(format!("Process PID {pid} not found"));
        return;
    };

    let name = process.name().to_string();
    let user = process
        .user_id()
        .and_then(|uid| state.user_cache.get(uid))
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let exe = process
        .exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());

    enter_env_view(
        state,
        return_view,
        "PROCESS ENV",
        format!("Process: {name}"),
        format!("PID: {pid}"),
        format!("User: {user}"),
        format!("Path: {exe}"),
    );
    match process::load_process_env(pid) {
        Ok(envs) => state.env_vars = envs,
        Err(err) => {
            state.env_vars = vec![format!("Failed to load env: {err}")];
        }
    }
}

pub(crate) fn enter_env_view(
    state: &mut AppState,
    return_view: ViewMode,
    title: &str,
    info_left1: String,
    info_right1: String,
    info_left2: String,
    info_right2: String,
) {
    state.input_mode = InputMode::Normal;
    state.env_return_view = return_view;
    state.env_modal_open = true;
    state.env_title = title.to_string();
    state.env_info_left1 = info_left1;
    state.env_info_right1 = info_right1;
    state.env_info_left2 = info_left2;
    state.env_info_right2 = info_right2;
    state.env_selected = 0;
    state.env_modal_hover = false;
}

fn format_ports_line(port_public: &str, port_internal: &str) -> String {
    if port_internal != "-" {
        format!("Ports: {port_public} | Int: {port_internal}")
    } else {
        format!("Ports: {port_public}")
    }
}
