use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sysinfo::System;

use crate::app::actions::{
    kill_selected_port_process, kill_selected_process, open_selected_container,
    open_selected_container_env, open_selected_container_logs,
};
use crate::app::state::{view_for_sidebar_index, Focus, InputMode, SortBy, ViewMode};
use crate::app::AppState;

pub(crate) fn handle_key_event(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if state.view_mode == ViewMode::DockerEnv {
        return handle_docker_env_mode(key, state);
    }

    match state.input_mode {
        InputMode::Normal => handle_normal_mode(key, state, system),
        InputMode::Filter => handle_filter_mode(key, state),
    }
}

fn handle_normal_mode(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    let list_len = match state.view_mode {
        ViewMode::Process => state.visible_pids.len(),
        ViewMode::Docker => state.visible_containers.len(),
        ViewMode::DockerEnv => 0,
        ViewMode::Ports => state.visible_ports.len(),
    };

    if matches!(key.code, KeyCode::Left | KeyCode::Right) {
        state.focus = match state.focus {
            Focus::Sidebar => Focus::Main,
            Focus::Main => Focus::Sidebar,
        };
        return false;
    }

    if state.focus == Focus::Sidebar {
        match key.code {
            KeyCode::Up => {
                if state.sidebar_index > 0 {
                    state.sidebar_index -= 1;
                }
                state.set_view(view_for_sidebar_index(state.sidebar_index));
            }
            KeyCode::Down => {
                if state.sidebar_index < 2 {
                    state.sidebar_index += 1;
                }
                state.set_view(view_for_sidebar_index(state.sidebar_index));
            }
            KeyCode::Enter => {
                state.set_view(view_for_sidebar_index(state.sidebar_index));
                state.focus = Focus::Main;
            }
            _ => {}
        }
        if matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::Enter) {
            return false;
        }
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('/') => {
            state.input_mode = InputMode::Filter;
        }
        KeyCode::Char('c') => {
            state.toggle_sort(SortBy::Cpu);
        }
        KeyCode::Char('m') => {
            state.toggle_sort(SortBy::Memory);
        }
        KeyCode::Char('n') => {
            state.toggle_sort(SortBy::Name);
        }
        KeyCode::Char('r') => {
            state.sort_order = state.sort_order.toggle();
        }
        KeyCode::Char('z') => {
            if state.view_mode == ViewMode::Process {
                state.zoom = !state.zoom;
                let label = if state.zoom { "ON" } else { "OFF" };
                state.set_message(format!("Zoom: {label}"));
            } else {
                state.set_message("Zoom only available in process view");
            }
        }
        KeyCode::Char('x') => {
            if !state.active_filter().is_empty() {
                state.active_filter_mut().clear();
                state.input_mode = InputMode::Normal;
                state.set_message("Search cleared");
            }
        }
        KeyCode::Char('d') => {
            let view = match state.view_mode {
                ViewMode::Process => ViewMode::Docker,
                ViewMode::Docker => ViewMode::Process,
                ViewMode::DockerEnv => ViewMode::Docker,
                ViewMode::Ports => ViewMode::Docker,
            };
            state.set_view(view);
            state.focus = Focus::Main;
            let label = match state.view_mode {
                ViewMode::Process => "Process",
                ViewMode::Docker => "Docker",
                ViewMode::DockerEnv => "Docker Env",
                ViewMode::Ports => "Ports",
            };
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('p') => {
            let view = match state.view_mode {
                ViewMode::Ports => ViewMode::Process,
                ViewMode::DockerEnv => ViewMode::Docker,
                _ => ViewMode::Ports,
            };
            state.set_view(view);
            state.focus = Focus::Main;
            let label = match state.view_mode {
                ViewMode::Process => "Process",
                ViewMode::Docker => "Docker",
                ViewMode::DockerEnv => "Docker Env",
                ViewMode::Ports => "Ports",
            };
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('k') => {
            if state.view_mode == ViewMode::Process {
                kill_selected_process(state, system);
            } else if state.view_mode == ViewMode::Ports {
                kill_selected_port_process(state, system);
            } else {
                state.set_message("Kill disabled in this view");
            }
        }
        KeyCode::Enter => {
            if state.view_mode == ViewMode::Docker {
                open_selected_container(state);
            }
        }
        KeyCode::Char('l') => {
            if state.view_mode == ViewMode::Docker {
                open_selected_container_logs(state);
            } else {
                state.set_message("Logs only available in Docker view");
            }
        }
        KeyCode::Char('e') => {
            if state.view_mode == ViewMode::Docker {
                open_selected_container_env(state);
            } else {
                state.set_message("Env only available in Docker view");
            }
        }
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.selected + 1 < list_len {
                state.selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.selected = state.selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            state.selected = (state.selected + 10).min(list_len.saturating_sub(1));
        }
        _ => {}
    }

    false
}

fn handle_filter_mode(key: KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Backspace => {
            state.active_filter_mut().pop();
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                state.active_filter_mut().push(ch);
            }
        }
        _ => {}
    }

    false
}

fn handle_docker_env_mode(key: KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => {
            state.set_view(ViewMode::Docker);
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Up => {
            if state.docker_env_selected > 0 {
                state.docker_env_selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.docker_env_selected + 1 < state.docker_env_vars.len() {
                state.docker_env_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.docker_env_selected = state.docker_env_selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            if !state.docker_env_vars.is_empty() {
                state.docker_env_selected =
                    (state.docker_env_selected + 10).min(state.docker_env_vars.len() - 1);
            }
        }
        _ => {}
    }
    false
}
