use sysinfo::{Pid, System};

use crate::app::{AppState, InputMode, ViewMode};
use crate::system::{docker, process};

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

pub(crate) fn open_selected_container(state: &mut AppState) {
    let Some(container_id) = state.visible_containers.get(state.selected) else {
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
    let Some(container_id) = state.visible_containers.get(state.selected) else {
        state.set_message("No container selected");
        return;
    };

    match docker::open_container_logs(container_id) {
        Ok(()) => {
            state.set_message(format!("Opening logs for {container_id}"));
        }
        Err(err) => {
            state.set_message(format!("Failed to open logs: {err}"));
        }
    }
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
    let Some(container_id) = state.visible_containers.get(state.selected).cloned() else {
        state.set_message("No container selected");
        return;
    };
    let name = state
        .visible_container_names
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| container_id.clone());
    let compose_name = state
        .visible_container_group_name
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let compose_path = state
        .visible_container_group_path
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let port_public = state
        .visible_container_ports_public
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let port_internal = state
        .visible_container_ports_internal
        .get(state.selected)
        .cloned()
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

fn enter_env_view(
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
    state.view_mode = ViewMode::DockerEnv;
    state.env_title = title.to_string();
    state.env_info_left1 = info_left1;
    state.env_info_right1 = info_right1;
    state.env_info_left2 = info_left2;
    state.env_info_right2 = info_right2;
    state.env_selected = 0;
}

fn format_ports_line(port_public: &str, port_internal: &str) -> String {
    if port_internal != "-" {
        format!("Ports: {port_public} | Int: {port_internal}")
    } else {
        format!("Ports: {port_public}")
    }
}
