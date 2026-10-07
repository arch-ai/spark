use sysinfo::{Pid, System};

use crate::app::state::{LogOutputMode, LogSource, OperationComplete};
use crate::app::{AppState, InputMode, ViewMode};
use crate::system::{docker, node, process};

/// Check if a PID is managed by PM2 and return the PM2 ID if found
fn find_pm2_id_for_pid(pid: u32) -> Option<u32> {
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
        || parent_name.contains("node-dev")
    {
        Some((parent_pid, parent.name().to_string()))
    } else {
        None
    }
}

pub(crate) fn kill_selected_process(state: &mut AppState, _system: &mut System) {
    let Some(pid) = state
        .visible_pids
        .get(state.selected)
        .copied()
        .filter(|pid| pid.as_u32() != 0)
    else {
        state.set_message("No process selected");
        return;
    };
    start_process_kill(state, pid.as_u32());
}

pub(crate) fn kill_selected_port_process(state: &mut AppState, _system: &mut System) {
    if let Some(id) = state
        .visible_ports_container_ids
        .get(state.selected)
        .and_then(Clone::clone)
    {
        start_container_kills(state, vec![id]);
        return;
    }
    match state
        .visible_ports
        .get(state.selected)
        .copied()
        .filter(|pid| pid.as_u32() != 0)
    {
        Some(pid) => start_process_kill(state, pid.as_u32()),
        None => state.set_message("No accessible process associated with this port"),
    }
}

/// Queue one action per target and deliver its actual result to the UI thread.
pub(crate) fn start_background_action(
    state: &mut AppState,
    key: String,
    label: String,
    command: impl FnOnce() -> std::io::Result<String> + Send + 'static,
) {
    if state.pending_operations.contains_key(&key) {
        state.set_message("An action is already running for this target");
        return;
    }
    state.pending_operations.insert(key.clone(), false);
    state.set_message(format!("{label}..."));
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let message = result.unwrap_or_else(|err| format!("{label} failed:\n\n{err}"));
        let _ = tx.send(OperationComplete {
            request_id: None,
            container_id: key,
            success,
            message,
            output: None,
        });
    });
}

pub(crate) fn start_pm2_action(
    state: &mut AppState,
    pm_id: u32,
    name: String,
    action: crate::app::ContextMenuAction,
) {
    use crate::app::ContextMenuAction;
    let (verb, command): (&str, fn(u32) -> std::io::Result<()>) = match action {
        ContextMenuAction::Start => ("Starting", node::pm2_start),
        ContextMenuAction::Stop => ("Stopping", node::pm2_stop),
        ContextMenuAction::Restart => ("Restarting", node::pm2_restart),
        _ => return,
    };
    start_background_action(
        state,
        format!("pm2::{pm_id}"),
        format!("{verb} PM2 {name}"),
        move || {
            command(pm_id)?;
            Ok(format!("PM2 {name}: action completed"))
        },
    );
}

pub(crate) fn start_process_kill(state: &mut AppState, pid: u32) {
    // Capture the kernel start tick before background work can delay the action.
    let identity = match process::process_identity(pid) {
        Ok(identity) => identity,
        Err(err) => {
            state.set_message(format!("Cannot access PID {pid}: {err}"));
            return;
        }
    };
    if state.view_mode == ViewMode::Process
        && state
            .process_identities
            .get(&pid)
            .is_some_and(|expected| *expected != identity)
    {
        state.set_message("Selected process exited. Refresh before retrying.");
        return;
    }
    start_background_action(
        state,
        format!("process::{pid}"),
        format!("Stopping PID {pid}"),
        move || {
            use sysinfo::Signal;
            let mut system = System::new();
            system.refresh_processes_specifics(
                sysinfo::ProcessRefreshKind::new()
                    .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
                    .with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
            );
            if process::process_identity(pid)? != identity {
                return Err(std::io::Error::other(
                    "The selected process has exited; PID was reused",
                ));
            }
            let target = Pid::from_u32(pid);
            let selected = system
                .process(target)
                .ok_or_else(|| std::io::Error::other("Process no longer exists"))?;
            // Capture both targets before a PM2 lookup can delay the action.
            let (signal_target, name) = find_supervisor_parent(target, &system)
                .unwrap_or_else(|| (target, selected.name().to_string()));
            let target_identity = if signal_target == target {
                identity
            } else {
                process::process_identity(signal_target.as_u32())?
            };
            // Query PM2 only for Node processes, outside the UI thread.
            if selected.name().contains("node") {
                if let Some(pm_id) = find_pm2_id_for_pid(pid) {
                    if process::process_identity(pid)? != identity {
                        return Err(std::io::Error::other("The selected process has exited"));
                    }
                    node::pm2_stop(pm_id)?;
                    return Ok(format!("Stopped PM2 {pm_id} (PID {pid})"));
                }
            }
            if process::process_identity(pid)? != identity
                || process::process_identity(signal_target.as_u32())? != target_identity
            {
                return Err(std::io::Error::other(
                    "The selected process or its supervisor has exited; refresh before retrying",
                ));
            }
            let target = signal_target;
            let selected = system.process(target).unwrap();
            if !selected.kill_with(Signal::Term).unwrap_or(false) {
                return Err(std::io::Error::other(
                    "Permission denied or process already exited",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            // One recheck before escalation; never signal a replacement with this PID.
            if process::process_identity(target.as_u32()).ok() == Some(target_identity) {
                system.refresh_processes();
                if let Some(remaining) = system.process(target) {
                    if process::process_identity(target.as_u32()).ok() == Some(target_identity)
                        && !remaining.kill_with(Signal::Kill).unwrap_or(false)
                    {
                        return Err(std::io::Error::other("Unable to stop process"));
                    }
                }
            }
            Ok(format!("Stopped {name} (PID {target})"))
        },
    );
}

pub(crate) fn kill_selected_in_docker(state: &mut AppState) {
    use crate::system::docker::DockerRow;
    let targets: Vec<String> = match state.docker_rows.get(state.docker_selected_row) {
        Some(DockerRow::Item { index, .. }) => state
            .visible_containers
            .get(*index)
            .cloned()
            .into_iter()
            .collect(),
        Some(DockerRow::Group { name, path, .. }) => state
            .visible_containers
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                let candidate_path = state
                    .visible_container_group_path
                    .get(*index)
                    .map(String::as_str)
                    .filter(|value| *value != "-" && !value.is_empty());
                candidate_path == path.as_deref()
                    && (path.is_some()
                        || state
                            .visible_container_group_name
                            .get(*index)
                            .is_some_and(|candidate| candidate == name))
            })
            .map(|(_, id)| id.clone())
            .collect(),
        _ => Vec::new(),
    };
    start_container_kills(state, targets);
}

fn start_container_kills(state: &mut AppState, targets: Vec<String>) {
    let targets: Vec<_> = targets
        .into_iter()
        .filter(|id| !state.pending_operations.contains_key(id))
        .collect();
    if targets.is_empty() {
        state.set_message("No available containers selected");
        return;
    }
    for id in &targets {
        state.pending_operations.insert(id.clone(), false);
    }
    state.set_message(format!("Killing {} container(s)...", targets.len()));
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        for id in targets {
            let result = docker::kill_container(&id);
            let success = result.is_ok();
            let message = match result {
                Ok(()) => format!("Killed container {}", id),
                Err(err) => format!("Failed to kill container: {err}"),
            };
            let _ = tx.send(OperationComplete {
                request_id: None,
                container_id: id,
                success,
                message,
                output: None,
            });
        }
    });
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

pub(crate) fn start_log_fetch<F>(state: &mut AppState, title: String, source: LogSource, command: F)
where
    F: FnOnce() -> std::io::Result<String> + Send + 'static,
{
    state.log_request_id = state.log_request_id.wrapping_add(1);
    state.log_in_progress = Some(title.clone());
    state.log_output = None;
    state.log_output_hover = false;
    state.log_select_hover = false;
    state.log_select_mode = false;
    state.log_output_mode = LogOutputMode::Logs;
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
    let request_id = Some(state.log_request_id);
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = match result {
            Ok(output) => output,
            Err(err) => err.to_string(),
        };
        let _ = tx.send(OperationComplete {
            request_id,
            container_id: format!("logs::{}", title),
            success,
            message: if success {
                String::new()
            } else {
                output.clone()
            },
            output: Some(output),
        });
    });
}

pub(crate) fn start_inspect_fetch<F>(state: &mut AppState, title: String, command: F)
where
    F: FnOnce() -> std::io::Result<String> + Send + 'static,
{
    state.log_request_id = state.log_request_id.wrapping_add(1);
    state.log_in_progress = Some(title.clone());
    state.log_output = None;
    state.log_output_hover = false;
    state.log_select_hover = false;
    state.log_select_mode = false;
    state.log_output_mode = LogOutputMode::Inspect;
    state.log_text.clear();
    state.log_lines.clear();
    state.log_wrap_width = 0;
    state.log_line_count = 0;
    state.log_scroll = 0;
    state.log_follow = false;
    state.log_last_scroll = std::time::Instant::now();
    state.log_source = None;
    state.log_refresh_in_progress = false;
    state.log_last_refresh = std::time::Instant::now();

    let request_id = Some(state.log_request_id);
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = result.as_ref().ok().cloned();
        let message = result
            .as_ref()
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        let _ = tx.send(OperationComplete {
            request_id,
            container_id: format!("inspect::{}", title),
            success,
            message,
            output,
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
    let request_id = Some(state.log_request_id);
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = match result {
            Ok(output) => output,
            Err(err) => err.to_string(),
        };
        let _ = tx.send(OperationComplete {
            request_id,
            container_id: format!("logs::{}", title),
            success,
            message: if success {
                String::new()
            } else {
                output.clone()
            },
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
        ViewMode::DockerEnv | ViewMode::Projects => {}
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
    start_container_env_fetch(state, container_id);
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
            start_container_env_fetch(state, id);
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
    _system: &System,
    pid: Pid,
    return_view: ViewMode,
) {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| format!("PID {pid}"));
    let uid = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Uid:")
                    .and_then(|ids| ids.split_whitespace().next())
                    .and_then(|id| id.parse::<sysinfo::Uid>().ok())
            })
        });
    let user = uid
        .as_ref()
        .and_then(|uid| state.user_cache.get(uid))
        .cloned()
        .or_else(|| uid.map(|uid| uid.to_string()))
        .unwrap_or_else(|| "-".into());
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "-".into());

    enter_env_view(
        state,
        return_view,
        "PROCESS ENV",
        format!("Process: {name}"),
        format!("PID: {pid}"),
        format!("User: {user}"),
        format!("Path: {exe}"),
    );
    start_process_env_fetch(state, pid);
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
    state.env_request = None;
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

pub(crate) fn start_env_fetch(
    state: &mut AppState,
    command: impl FnOnce() -> std::io::Result<Vec<String>> + Send + 'static,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    state.env_vars = vec!["Loading environment...".into()];
    state.env_request = Some(rx);
    std::thread::spawn(move || {
        let _ = tx.send(command());
    });
}

pub(crate) fn start_process_env_fetch(state: &mut AppState, pid: Pid) {
    start_env_fetch(state, move || process::load_process_env(pid));
}

pub(crate) fn start_container_env_fetch(state: &mut AppState, container_id: String) {
    start_env_fetch(state, move || docker::load_container_env(&container_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_owner_is_available_without_a_ui_process_scan() {
        let mut state = AppState::new();
        let uid: sysinfo::Uid = unsafe { libc::getuid() }.to_string().parse().unwrap();
        state.user_cache.insert(uid, "qa-owner".into());
        open_process_env_for_pid(
            &mut state,
            &System::new(),
            Pid::from_u32(std::process::id()),
            ViewMode::Process,
        );
        assert_eq!(state.env_info_left2, "User: qa-owner");
    }
    #[test]
    fn pending_actions_do_not_block_input_or_start_duplicate_work() {
        let mut state = AppState::new();
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, entry) = std::sync::mpsc::channel();
        start_background_action(
            &mut state,
            "pm2::7".into(),
            "Restarting PM2 api".into(),
            move || {
                entered.send(()).unwrap();
                wait.recv().unwrap();
                Ok("Restarted api".into())
            },
        );
        entry
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        start_background_action(
            &mut state,
            "pm2::7".into(),
            "Restarting PM2 api".into(),
            || panic!("duplicate action must not execute"),
        );
        assert_eq!(state.pending_operations.len(), 1);
        state.set_view(ViewMode::Ports);
        assert_eq!(state.view_mode, ViewMode::Ports);
        release.send(()).unwrap();
        let result = state
            .operation_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.message, "Restarted api");
    }
    #[test]
    fn a_stale_process_identity_prevents_signaling_a_reused_pid() {
        let mut state = AppState::new();
        let pid = std::process::id();
        state
            .process_identities
            .insert(pid, process::process_identity(pid).unwrap() + 1);
        start_process_kill(&mut state, pid);
        assert!(state.pending_operations.is_empty());
        assert!(state
            .message
            .as_ref()
            .unwrap()
            .contains("Selected process exited"));
    }
}
