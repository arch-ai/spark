use sysinfo::{Pid, System};

use crate::app::{AppState, InputMode, ViewMode};
use crate::system::docker;

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

pub(crate) fn open_selected_container_env(state: &mut AppState) {
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

    state.input_mode = InputMode::Normal;
    state.set_view(ViewMode::DockerEnv);
    state.docker_env_name = Some(name);
    state.docker_env_compose_name = compose_name;
    state.docker_env_compose_path = compose_path;
    state.docker_env_port_public = port_public;
    state.docker_env_port_internal = port_internal;
    state.docker_env_selected = 0;
    match docker::load_container_env(&container_id) {
        Ok(envs) => state.docker_env_vars = envs,
        Err(err) => {
            state.docker_env_vars = vec![format!("Failed to load env: {err}")];
        }
    }
}
