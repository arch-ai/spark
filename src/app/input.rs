use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use sysinfo::System;

use crate::app::actions::{
    enter_env_view, kill_selected_in_docker, kill_selected_port_process, kill_selected_process,
    open_selected_container, open_selected_container_logs, open_selected_env, start_inspect_fetch,
    start_log_fetch,
};
use crate::app::state::{
    view_for_sidebar_index, ContextMenu, ContextMenuAction, ContextMenuTarget, DeleteConfirm,
    DeleteConfirmChoice, DeleteKind, 
    DockerDfKind, DockerListKind, Focus, InputMode, LogOutputMode, LogSource, OperationComplete,
    PruneConfirmChoice, SortBy, ViewMode,
};
use crate::app::AppState;
use crate::system::docker::{ContainerInfo, DockerListItem, DockerRow};
use crate::system::node::open_path_location;
use crate::system::process::{self, load_process_logs};

pub(crate) fn handle_key_event(
    key: KeyEvent,
    state: &mut AppState,
    system: &mut System,
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if let Some(confirm) = state.pending_delete.clone() {
        match key.code {
            KeyCode::Char('y') => {
                state.pending_delete = None;
                state.pending_delete_hover = None;
                start_delete_action(state, confirm);
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.pending_delete = None;
                state.pending_delete_hover = None;
                state.set_message("Delete canceled.");
            }
            _ => {}
        }
        return false;
    }
    if let Some(action) = state.pending_prune {
        match key.code {
            KeyCode::Char('y') => {
                state.pending_prune = None;
                state.pending_prune_hover = None;
                start_prune_action(state, action);
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.pending_prune = None;
                state.pending_prune_hover = None;
                state.set_message("Prune canceled.");
            }
            _ => {}
        }
        return false;
    }
    if state.prune_output.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            state.prune_output = None;
            state.prune_output_hover = false;
        }
        return false;
    }
    if state.log_output.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            state.clear_log_state();
        } else if matches!(key.code, KeyCode::Char('v')) && state.log_output_mode == LogOutputMode::Logs {
            toggle_log_select_mode(state);
        } else if let Some((viewport_w, viewport_h)) = log_modal_inner_size(state) {
            match key.code {
                KeyCode::Up => {
                    apply_log_scroll(state, -1, viewport_w, viewport_h);
                }
                KeyCode::Down => {
                    apply_log_scroll(state, 1, viewport_w, viewport_h);
                }
                KeyCode::PageUp => {
                    let delta = viewport_h.saturating_sub(1) as i32;
                    apply_log_scroll(state, -(delta.max(1)), viewport_w, viewport_h);
                }
                KeyCode::PageDown => {
                    let delta = viewport_h.saturating_sub(1) as i32;
                    apply_log_scroll(state, delta.max(1), viewport_w, viewport_h);
                }
                KeyCode::Home => {
                    state.log_scroll = 0;
                    state.log_follow = false;
                    state.log_last_scroll = std::time::Instant::now();
                }
                KeyCode::End => {
                    state.log_follow = true;
                    state.log_scroll = state.log_max_scroll(viewport_w, viewport_h);
                    state.log_last_scroll = std::time::Instant::now();
                }
                _ => {}
            }
        }
        return false;
    }
    if state.docker_list_open {
        return handle_docker_list_modal_mode(key, state);
    }
    if state.env_modal_open {
        return handle_env_modal_mode(key, state);
    }
    if state.log_in_progress.is_some() {
        return false;
    }
    // Close context menu on Escape
    if state.context_menu.is_some() && key.code == KeyCode::Esc {
        state.context_menu = None;
        return false;
    }

    if state.view_mode == ViewMode::DockerEnv {
        return handle_env_mode(key, state);
    }

    match state.input_mode {
        InputMode::Normal => handle_normal_mode(key, state, system, pm2_view, pm2_rows),
        InputMode::Filter => handle_filter_mode(key, state),
    }
}

fn handle_normal_mode(
    key: KeyEvent,
    state: &mut AppState,
    system: &mut System,
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) -> bool {
    let list_len = match state.view_mode {
        ViewMode::Process => state.visible_pids.len(),
        ViewMode::Docker => state.visible_containers.len(),
        ViewMode::DockerEnv => 0,
        ViewMode::Ports => state.visible_ports.len(),
        ViewMode::Node => state.visible_pids.len(),
    };

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if state.view_mode == ViewMode::Docker {
            match key.code {
                KeyCode::Char('b') => {
                    request_prune_confirmation(state, ContextMenuAction::PruneBuildCache);
                }
                KeyCode::Char('i') => {
                    request_prune_confirmation(state, ContextMenuAction::PruneDanglingImages);
                }
                KeyCode::Char('o') => {
                    request_prune_confirmation(state, ContextMenuAction::PruneVolumes);
                }
                _ => {}
            }
        } else if state.view_mode == ViewMode::Node && state.pm2_available {
            if let Some(pm2_idx) = state.pm2_hover_row {
                if let Some(proc) = pm2_rows.get(pm2_idx).and_then(|idx| pm2_view.get(*idx)) {
                    match key.code {
                        KeyCode::Char('r') => {
                            state.set_message(format!("Restarting PM2 {}...", proc.name));
                            let _ = crate::system::node::pm2_restart(proc.pm_id);
                        }
                        KeyCode::Char('s') => {
                            state.set_message(format!("Stopping PM2 {}...", proc.name));
                            let _ = crate::system::node::pm2_stop(proc.pm_id);
                        }
                        KeyCode::Char('t') => {
                            state.set_message(format!("Starting PM2 {}...", proc.name));
                            let _ = crate::system::node::pm2_start(proc.pm_id);
                        }
                        KeyCode::Char('o') => {
                            if let Some(path) = proc.script.as_deref() {
                                let dir = std::path::Path::new(path)
                                    .parent()
                                    .unwrap_or_else(|| std::path::Path::new(path));
                                if let Err(err) = crate::system::node::open_path_location(dir) {
                                    state.set_message(format!("Failed to open dir: {}", err));
                                } else {
                                    state.set_message("Opened script location.");
                                }
                            } else {
                                state.set_message("No script path for this PM2 process.");
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        return false;
    }

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
                if state.sidebar_index < 3 {
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
                ViewMode::Node => ViewMode::Docker,
            };
            state.set_view(view);
            state.focus = Focus::Main;
            let label = view_label(state.view_mode);
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
            let label = view_label(state.view_mode);
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('j') => {
            let view = match state.view_mode {
                ViewMode::Node => ViewMode::Process,
                _ => ViewMode::Node,
            };
            state.set_view(view);
            state.focus = Focus::Main;
            let label = view_label(state.view_mode);
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('k') => {
            if state.view_mode == ViewMode::Process || state.view_mode == ViewMode::Node {
                kill_selected_process(state, system);
            } else if state.view_mode == ViewMode::Docker {
                kill_selected_in_docker(state);
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
            open_selected_env(state, system);
        }
        KeyCode::Up => {
            if state.view_mode == ViewMode::Ports {
                move_ports_selection(state, -1);
            } else if state.view_mode == ViewMode::Node {
                move_node_selection(state, -1);
            } else if state.view_mode == ViewMode::Docker {
                move_docker_selection(state, -1);
            } else if state.selected > 0 {
                state.selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.view_mode == ViewMode::Ports {
                move_ports_selection(state, 1);
            } else if state.view_mode == ViewMode::Node {
                move_node_selection(state, 1);
            } else if state.view_mode == ViewMode::Docker {
                move_docker_selection(state, 1);
            } else if state.selected + 1 < list_len {
                state.selected += 1;
            }
        }
        KeyCode::PageUp => {
            if state.view_mode == ViewMode::Ports {
                for _ in 0..10 {
                    if !move_ports_selection(state, -1) {
                        break;
                    }
                }
            } else if state.view_mode == ViewMode::Node {
                for _ in 0..10 {
                    if !move_node_selection(state, -1) {
                        break;
                    }
                }
            } else if state.view_mode == ViewMode::Docker {
                for _ in 0..10 {
                    if !move_docker_selection(state, -1) {
                        break;
                    }
                }
            } else {
                state.selected = state.selected.saturating_sub(10);
            }
        }
        KeyCode::PageDown => {
            if state.view_mode == ViewMode::Ports {
                for _ in 0..10 {
                    if !move_ports_selection(state, 1) {
                        break;
                    }
                }
            } else if state.view_mode == ViewMode::Node {
                for _ in 0..10 {
                    if !move_node_selection(state, 1) {
                        break;
                    }
                }
            } else if state.view_mode == ViewMode::Docker {
                for _ in 0..10 {
                    if !move_docker_selection(state, 1) {
                        break;
                    }
                }
            } else {
                state.selected = (state.selected + 10).min(list_len.saturating_sub(1));
            }
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

fn handle_env_mode(key: KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => {
            state.view_mode = state.env_return_view;
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Up => {
            if state.env_selected > 0 {
                state.env_selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.env_selected + 1 < state.env_vars.len() {
                state.env_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.env_selected = state.env_selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            if !state.env_vars.is_empty() {
                state.env_selected = (state.env_selected + 10).min(state.env_vars.len() - 1);
            }
        }
        _ => {}
    }
    false
}

fn handle_env_modal_mode(key: KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            state.env_modal_open = false;
            state.env_modal_hover = false;
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Up => {
            if state.env_selected > 0 {
                state.env_selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.env_selected + 1 < state.env_vars.len() {
                state.env_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.env_selected = state.env_selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            if !state.env_vars.is_empty() {
                state.env_selected = (state.env_selected + 10).min(state.env_vars.len() - 1);
            }
        }
        _ => {}
    }
    false
}

fn handle_docker_list_modal_mode(key: KeyEvent, state: &mut AppState) -> bool {
    let total = state.docker_list_items.len();
    match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            state.docker_list_open = false;
            state.docker_list_hover = false;
            state.context_menu = None;
        }
        KeyCode::Up => {
            if total > 0 && state.docker_list_selected > 0 {
                state.docker_list_selected -= 1;
            }
        }
        KeyCode::Down => {
            if total > 0 && state.docker_list_selected + 1 < total {
                state.docker_list_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.docker_list_selected = state.docker_list_selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            if total > 0 {
                state.docker_list_selected = (state.docker_list_selected + 10).min(total - 1);
            }
        }
        KeyCode::Home => {
            if total > 0 {
                state.docker_list_selected = 0;
            }
        }
        KeyCode::End => {
            if total > 0 {
                state.docker_list_selected = total - 1;
            }
        }
        _ => {}
    }
    false
}

fn move_ports_selection(state: &mut AppState, direction: isize) -> bool {
    if direction == 0 {
        return false;
    }
    let len = state.visible_ports.len() as isize;
    if len == 0 {
        return false;
    }
    let mut idx = state.selected as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return false;
        }
        let next = idx as usize;
        if !state.is_ports_group_row(next) {
            state.selected = next;
            return true;
        }
    }
}

fn move_node_selection(state: &mut AppState, direction: isize) -> bool {
    if direction == 0 {
        return false;
    }
    let len = state.visible_pids.len() as isize;
    if len == 0 {
        return false;
    }
    let mut idx = state.selected as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return false;
        }
        let next = idx as usize;
        if state.is_node_selectable_row(next) {
            state.selected = next;
            return true;
        }
    }
}

fn move_docker_selection(state: &mut AppState, direction: isize) -> bool {
    if direction == 0 {
        return false;
    }
    let len = state.docker_rows.len() as isize;
    if len == 0 {
        return false;
    }
    let mut idx = state.docker_selected_row as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return false;
        }
        let next = idx as usize;
        if state.is_docker_selectable_row(next) {
            state.docker_selected_row = next;
            return true;
        }
    }
}

fn view_label(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Process => "Processes",
        ViewMode::Docker => "Docker",
        ViewMode::DockerEnv => "Env",
        ViewMode::Ports => "Ports",
        ViewMode::Node => "Node.js",
    }
}

const SIDEBAR_WIDTH: u16 = 20;
// Sidebar layout: border(1) + logo(7) + spacing(1) = 9
const SIDEBAR_MENU_START_ROW: u16 = 9;

/// Returns true if a re-render is needed
pub(crate) fn handle_mouse_event(
    mouse: MouseEvent,
    state: &mut AppState,
    containers: &[crate::system::docker::ContainerInfo],
    ports: &[crate::system::ports::PortInfo],
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
    terminal_width: u16,
    terminal_height: u16,
) -> bool {
    let (width, height) = (terminal_width, terminal_height);
    let x = mouse.column;
    let y = mouse.row;

    // Check if sidebar is visible
    let show_sidebar = width >= SIDEBAR_WIDTH + 1 + 40; // sidebar + gap + min main
    let main_x = if show_sidebar { SIDEBAR_WIDTH + 1 } else { 0 };
    let main_width = if show_sidebar { width.saturating_sub(SIDEBAR_WIDTH + 1) } else { width };

    if state.pending_delete.is_some() {
        return handle_delete_confirm_mouse(mouse, state, main_x, main_width, height);
    }
    if state.pending_prune.is_some() {
        return handle_prune_confirm_mouse(mouse, state, main_x, main_width, height);
    }
    if state.prune_output.is_some() {
        return handle_prune_output_mouse(mouse, state, main_x, main_width, height);
    }
    if state.env_modal_open {
        return handle_env_modal_mouse(mouse, state, main_x, main_width, height);
    }
    if state.log_output.is_some() {
        if state.log_select_mode {
            return false;
        }
        return handle_log_output_mouse(mouse, state, main_x, main_width, height);
    }
    if state.log_in_progress.is_some() {
        return true;
    }
    if state.docker_list_open {
        if let Some(result) = handle_context_menu_mouse(mouse, state, containers, pm2_view, pm2_rows) {
            return result;
        }
        return handle_docker_list_modal_mouse(mouse, state, main_x, main_width, width, height);
    }

    if let Some(result) = handle_context_menu_mouse(mouse, state, containers, pm2_view, pm2_rows) {
        return result;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Clear hover on click
            state.hover_row = None;
            state.sidebar_hover = None;

            if show_sidebar && x < SIDEBAR_WIDTH {
                handle_sidebar_click(state, y);
            } else {
                handle_main_click(state, x.saturating_sub(main_x), y, main_width, height, pm2_rows);
            }
            true
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Right-click to open context menu
            match state.view_mode {
                ViewMode::Docker => {
                    handle_docker_right_click(state, x, y, width, height, main_x, containers);
                    true
                }
                ViewMode::Process => {
                    handle_process_right_click(state, x, y, width, height, main_x);
                    true
                }
                ViewMode::Ports => {
                    handle_ports_right_click(state, x, y, width, height, main_x, ports);
                    true
                }
                ViewMode::Node => {
                    handle_node_right_click(state, x, y, width, height, main_x, pm2_view, pm2_rows);
                    true
                }
                _ => false
            }
        }
        MouseEventKind::Moved => {
            // Throttle hover re-renders to avoid excessive CPU usage
            use std::time::Duration;
            const HOVER_RENDER_INTERVAL: Duration = Duration::from_millis(4); // ~120fps max

            if state.last_hover_render.elapsed() < HOVER_RENDER_INTERVAL {
                // Update pending hover row but don't trigger render yet
                if show_sidebar && x < SIDEBAR_WIDTH {
                    state.docker_df_hover = None;
                    handle_sidebar_hover(state, y);
                } else {
                handle_main_hover(state, x.saturating_sub(main_x), y, height, pm2_rows);
                }
                // Don't trigger render, will happen on next tick or when interval expires
                return false;
            }

            // Update hover state and trigger render
            if show_sidebar && x < SIDEBAR_WIDTH {
                // Hovering over sidebar
                let old_hover = state.sidebar_hover;
                let old_df_hover = state.docker_df_hover;
                state.hover_row = None;
                state.docker_df_hover = None;
                handle_sidebar_hover(state, y);
                state.sidebar_hover != old_hover || state.docker_df_hover != old_df_hover
            } else {
                let old_hover = state.hover_row;
                let old_df_hover = state.docker_df_hover;
                let old_pm2_hover = state.pm2_hover_row;
                state.sidebar_hover = None;
                handle_main_hover(state, x.saturating_sub(main_x), y, height, pm2_rows);
                state.hover_row != old_hover
                    || state.docker_df_hover != old_df_hover
                    || state.pm2_hover_row != old_pm2_hover
            }
        }
        MouseEventKind::ScrollUp => {
            // Scroll up = move selection up
            handle_scroll(state, -1);
            true
        }
        MouseEventKind::ScrollDown => {
            // Scroll down = move selection down
            handle_scroll(state, 1);
            true
        }
        _ => false
    }
}

fn handle_sidebar_click(state: &mut AppState, y: u16) {
    if y < SIDEBAR_MENU_START_ROW {
        return;
    }

    let menu_index = (y - SIDEBAR_MENU_START_ROW) as usize;
    // 4 menu items: Processes, Ports, Docker, Node JS
    if menu_index < 4 {
        state.sidebar_index = menu_index;
        state.set_view(view_for_sidebar_index(menu_index));
        state.focus = Focus::Main;
    }
}

fn handle_sidebar_hover(state: &mut AppState, y: u16) {
    if y < SIDEBAR_MENU_START_ROW {
        state.sidebar_hover = None;
        return;
    }

    let menu_index = (y - SIDEBAR_MENU_START_ROW) as usize;
    // 4 menu items
    if menu_index < 4 {
        state.sidebar_hover = Some(menu_index);
    } else {
        state.sidebar_hover = None;
    }
}

fn jump_to_top(state: &mut AppState) {
    match state.view_mode {
        ViewMode::Process | ViewMode::Ports | ViewMode::Node => {
            state.selected = 0;
        }
        ViewMode::Docker => {
            state.docker_selected_row = 0;
        }
        ViewMode::DockerEnv => {
            state.env_selected = 0;
        }
    }
    // Reset scroll to top
    match state.view_mode {
        ViewMode::Process => state.process_scroll = 0,
        ViewMode::Docker => state.docker_scroll = 0,
        ViewMode::Ports => state.ports_scroll = 0,
        ViewMode::Node => state.node_scroll = 0,
        ViewMode::DockerEnv => {}
    }
}

fn jump_to_bottom(state: &mut AppState) {
    match state.view_mode {
        ViewMode::Process => {
            if !state.visible_pids.is_empty() {
                state.selected = state.visible_pids.len() - 1;
            }
        }
        ViewMode::Docker => {
            if !state.docker_rows.is_empty() {
                state.docker_selected_row = state.docker_rows.len() - 1;
            }
        }
        ViewMode::Ports => {
            if !state.visible_ports.is_empty() {
                state.selected = state.visible_ports.len() - 1;
            }
        }
        ViewMode::Node => {
            if !state.visible_pids.is_empty() {
                state.selected = state.visible_pids.len() - 1;
            }
        }
        ViewMode::DockerEnv => {
            if !state.env_vars.is_empty() {
                state.env_selected = state.env_vars.len() - 1;
            }
        }
    }
}

fn handle_scroll(state: &mut AppState, direction: isize) {

    match state.view_mode {
        ViewMode::Process => {
            let len = state.visible_pids.len();
            if direction < 0 && state.selected > 0 {
                state.selected -= 1;
            } else if direction > 0 && state.selected + 1 < len {
                state.selected += 1;
            }
        }
        ViewMode::Docker => {
            move_docker_selection(state, direction);
        }
        ViewMode::Ports => {
            move_ports_selection(state, direction);
        }
        ViewMode::Node => {
            move_node_selection(state, direction);
        }
        ViewMode::DockerEnv => {
            if direction < 0 && state.env_selected > 0 {
                state.env_selected -= 1;
            } else if direction > 0 && state.env_selected + 1 < state.env_vars.len() {
                state.env_selected += 1;
            }
        }
    }
}

fn handle_main_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    pm2_rows: &[usize],
) {
    // Dismiss context menu if clicking elsewhere
    if state.context_menu.is_some() {
        state.context_menu = None;
        return;
    }

    // Check for click on search box area (rows 4-6 for most views)
    // Layout: title(3) + header(1) = 4, then search box starts
    let search_start: u16 = 4;
    let search_end: u16 = 7; // search box is 3 rows
    let in_search_area = y >= search_start && y < search_end && state.view_mode != ViewMode::DockerEnv;

    if in_search_area {
        state.input_mode = InputMode::Filter;
        return;
    } else if state.input_mode == InputMode::Filter {
        // Clicking outside search box exits filter mode
        state.input_mode = InputMode::Normal;
    }

    // Row offsets for ratatui layout:
    // Process: 3 (title) + 1 (header) + 3 (search) + 4 (bars) + 2 (table border+header) = 13
    // Docker: 3 (title) + 1 (header) + 3 (search) + 7 (df stats) + 2 (table border+header) = 16
    // Ports/Node: 3 + 1 + 3 + 2 = 9
    let list_start: u16 = match state.view_mode {
        ViewMode::Process => 13,
        ViewMode::Docker => 16,
        ViewMode::Ports => 9,
        ViewMode::Node => {
            if state.pm2_available {
                let (pm2_start, pm2_height, node_start, node_height) = node_table_bounds(height);
                if pm2_height > 0 && y >= pm2_start && y < pm2_start + pm2_height {
                    return;
                }
                if node_height > 0 && y >= node_start && y < node_start + node_height {
                    node_start + 2
                } else {
                    return;
                }
            } else {
                9
            }
        }
        ViewMode::DockerEnv => {
            // EnvView: 3 (title) + 5 (info) + 2 (table border+header) = 10
            if y >= 10 {
                let clicked_row = (y - 10) as usize;
                if clicked_row < state.env_vars.len() {
                    state.env_selected = clicked_row;
                }
            }
            return;
        }
    };

    // Calculate table area bounds for nav icon detection
    let footer_height = 2u16; // help bar
    let table_top = list_start.saturating_sub(2); // include border+header
    let table_bottom = height.saturating_sub(footer_height);

    // Check for nav icon clicks (right edge of table area)
    if x >= width.saturating_sub(4) {
        // Click on top nav icon (▲) - jump to top
        if y == table_top {
            jump_to_top(state);
            return;
        }
        // Click on bottom nav icon (▼) - jump to bottom
        if y == table_bottom.saturating_sub(1) {
            jump_to_bottom(state);
            return;
        }
    }

    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;

    // Calculate max visible rows based on terminal height
    // Footer is 2 rows (help bar)
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        return;
    }

    match state.view_mode {
        ViewMode::Process => {
            let total = state.visible_pids.len();
            let target_row = state.process_scroll + clicked_visual_row;
            if target_row < total {
                state.selected = target_row;
            }
        }
        ViewMode::Docker => {
            let total = state.docker_rows.len();
            let target_row = state.docker_scroll + clicked_visual_row;
            if target_row < total && state.is_docker_selectable_row(target_row) {
                state.docker_selected_row = target_row;
            }
        }
        ViewMode::Ports => {
            let total = state.visible_ports.len();
            let target_row = state.ports_scroll + clicked_visual_row;
            if target_row < total && !state.is_ports_group_row(target_row) {
                state.selected = target_row;
            }
        }
        ViewMode::Node => {
            if state.pm2_available {
                let (pm2_start, pm2_height, node_start, node_height) = node_table_bounds(height);
                let pm2_list_start = pm2_start + 2;
                if pm2_height > 0 && y >= pm2_list_start && y < pm2_start + pm2_height {
                    let pm2_visible_height = pm2_height.saturating_sub(2) as usize;
                    if pm2_visible_height == 0 {
                        return;
                    }
                    let pm2_clicked_row = (y - pm2_list_start) as usize;
                    let pm2_target = state.pm2_scroll + pm2_clicked_row;
                    if pm2_target < pm2_rows.len() {
                        state.pm2_hover_row = Some(pm2_target);
                    } else {
                        state.pm2_hover_row = None;
                    }
                    return;
                }
                if node_height == 0 || y < node_start + 2 {
                    return;
                }
            }
            let total = state.visible_pids.len();
            let target_row = state.node_scroll + clicked_visual_row;
            if target_row < total && state.is_node_selectable_row(target_row) {
                state.selected = target_row;
            }
        }
        ViewMode::DockerEnv => {}
    }
}

fn handle_main_hover(
    state: &mut AppState,
    _x: u16,
    y: u16,
    height: u16,
    pm2_rows: &[usize],
) {
    // Docker df stats area: title(3) + header(1) + search(3) = 7, df stats is 7 rows
    // Data rows start at row 9 (0-indexed: 9, 10, 11, 12 for Images, Containers, Volumes, Build Cache)
    if state.view_mode == ViewMode::Docker && y >= 9 && y < 13 {
        let df_hover = (y - 9) as usize;
        if df_hover < 4 {
            state.docker_df_hover = Some(df_hover);
            state.hover_row = None;
            return;
        }
    } else if state.docker_df_hover.is_some() {
        state.docker_df_hover = None;
    }

    if state.view_mode != ViewMode::Node || !state.pm2_available {
        state.pm2_hover_row = None;
    }

    if state.view_mode == ViewMode::Node && state.pm2_available {
        let (pm2_start, pm2_height, node_start, node_height) = node_table_bounds(height);
        if pm2_height > 0 && y >= pm2_start && y < pm2_start + pm2_height {
            handle_pm2_hover(state, y, pm2_start, pm2_height, pm2_rows);
            state.hover_row = None;
            return;
        }
        state.pm2_hover_row = None;
        if node_height > 0 && y >= node_start && y < node_start + node_height {
            handle_node_hover(state, y, node_start, node_height);
            return;
        }
        state.hover_row = None;
        return;
    }

    // Row offsets for ratatui layout:
    // Process: 3 (title) + 1 (header) + 3 (search) + 4 (bars) + 2 (table border+header) = 13
    // Docker: 3 (title) + 1 (header) + 3 (search) + 7 (df stats) + 2 (table border+header) = 16
    // Ports/Node: 3 + 1 + 3 + 2 = 9
    let list_start: u16 = match state.view_mode {
        ViewMode::Process => 13,
        ViewMode::Docker => 16,
        ViewMode::Ports => 9,
        ViewMode::Node => 9,
        ViewMode::DockerEnv => {
            // EnvView: 3 (title) + 5 (info) + 2 (table border+header) = 10
            if y >= 10 {
                let hover = (y - 10) as usize;
                if hover < state.env_vars.len() {
                    state.hover_row = Some(hover);
                    return;
                }
            }
            state.hover_row = None;
            return;
        }
    };

    if y < list_start {
        state.hover_row = None;
        return;
    }

    let hovered_visual_row = (y - list_start) as usize;

    // Calculate visible height - footer is 2 rows (help bar)
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        state.hover_row = None;
        return;
    }

    match state.view_mode {
        ViewMode::Process => {
            let total = state.visible_pids.len();
            let target_row = state.process_scroll + hovered_visual_row;
            if target_row < total {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Docker => {
            let total = state.docker_rows.len();
            let target_row = state.docker_scroll + hovered_visual_row;
            if target_row < total {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Ports => {
            let total = state.visible_ports.len();
            let target_row = state.ports_scroll + hovered_visual_row;
            if target_row < total {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Node => {
            let total = state.visible_pids.len();
            let target_row = state.node_scroll + hovered_visual_row;
            if target_row < total && state.is_node_selectable_row(target_row) {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::DockerEnv => {}
    }
}

fn node_table_bounds(height: u16) -> (u16, u16, u16, u16) {
    let table_top = 7u16; // title(3) + header(1) + search(3)
    let help_height = 2u16;
    let available = height.saturating_sub(table_top + help_height);
    let mut pm2_height = available / 2;
    if pm2_height < 5 {
        pm2_height = available.min(5);
    }
    let mut node_height = available.saturating_sub(pm2_height);
    if node_height < 5 {
        let deficit = 5u16.saturating_sub(node_height);
        if pm2_height > deficit {
            pm2_height = pm2_height.saturating_sub(deficit);
            node_height = node_height.saturating_add(deficit);
        }
    }
    let pm2_start = table_top;
    let node_start = table_top + pm2_height;
    (pm2_start, pm2_height, node_start, node_height)
}

fn handle_pm2_hover(state: &mut AppState, y: u16, table_start: u16, table_height: u16, pm2_rows: &[usize]) {
    let list_start = table_start + 2;
    let visible_height = table_height.saturating_sub(2) as usize;
    if y < list_start || visible_height == 0 {
        state.pm2_hover_row = None;
        return;
    }
    let hovered_visual_row = (y - list_start) as usize;
    let target_row = state.pm2_scroll + hovered_visual_row;
    if target_row < pm2_rows.len() {
        state.pm2_hover_row = Some(target_row);
    } else {
        state.pm2_hover_row = None;
    }
}

fn handle_node_hover(state: &mut AppState, y: u16, table_start: u16, table_height: u16) {
    let list_start = table_start + 2;
    if y < list_start {
        state.hover_row = None;
        return;
    }
    let hovered_visual_row = (y - list_start) as usize;
    let visible_height = table_height.saturating_sub(2) as usize;
    if visible_height == 0 {
        state.hover_row = None;
        return;
    }
    let total = state.visible_pids.len();
    let target_row = state.node_scroll + hovered_visual_row;
    if target_row < total && state.is_node_selectable_row(target_row) {
        state.hover_row = Some(target_row);
    } else {
        state.hover_row = None;
    }
}

// Context menu constants
const MENU_WIDTH: u16 = 28;
const MENU_PADDING: u16 = 1;

/// Position context menu within terminal bounds
/// Returns (x, y) coordinates that ensure the menu is fully visible
fn position_context_menu(
    click_x: u16,
    click_y: u16,
    menu_item_count: usize,
    terminal_width: u16,
    terminal_height: u16,
) -> (u16, u16) {
    let menu_height = menu_item_count as u16 + MENU_PADDING * 2;

    // Horizontal positioning: prefer to show menu to the right of click
    // but shift left if it would overflow terminal width
    let menu_x = if click_x + MENU_WIDTH > terminal_width {
        terminal_width.saturating_sub(MENU_WIDTH)
    } else {
        click_x
    };

    // Vertical positioning: prefer to show menu below click
    // but shift up if it would overflow terminal height
    let menu_y = if click_y + menu_height > terminal_height {
        click_y.saturating_sub(menu_height)
    } else {
        click_y
    };

    (menu_x.max(0), menu_y.max(0))
}

fn handle_docker_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    _main_x: u16,
    containers: &[crate::system::docker::ContainerInfo],
) {
    if y >= 9 && y < 13 {
        let df_hover = (y - 9) as usize;
        let (target, items) = match df_hover {
            0 => (
                ContextMenuTarget::DockerDf { kind: DockerDfKind::Images },
                vec![ContextMenuAction::ShowImages, ContextMenuAction::PruneDanglingImages],
            ),
            1 => (
                ContextMenuTarget::DockerDf { kind: DockerDfKind::Containers },
                vec![ContextMenuAction::ShowContainers],
            ),
            3 => (
                ContextMenuTarget::DockerDf { kind: DockerDfKind::BuildCache },
                vec![ContextMenuAction::PruneBuildCache],
            ),
            2 => (
                ContextMenuTarget::DockerDf { kind: DockerDfKind::Volumes },
                vec![ContextMenuAction::ShowVolumes, ContextMenuAction::PruneVolumes],
            ),
            _ => return,
        };

        let (menu_x, menu_y) = position_context_menu(x, y, items.len(), width, height);
        state.context_menu = Some(ContextMenu {
            x: menu_x,
            y: menu_y,
            items,
            hover: Some(0),
            target,
            is_group: false,
            header: None,
        });
        return;
    }

    // Docker view: 3 (title) + 1 (header) + 3 (search) + 7 (df stats) + 2 (table border+header) = 16
    let list_start: u16 = 16;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        return;
    }

    // Use the same scroll offset as hover handler
    let scroll = state.docker_scroll;
    let total = state.docker_rows.len();

    let target_row = scroll + clicked_visual_row;
    if target_row >= total {
        return;
    }

    // Determine target and menu items based on row type
    let (target, items, is_group, header) = match &state.docker_rows[target_row] {
        DockerRow::Group { name, path, .. } => {
            let target = ContextMenuTarget::Group {
                name: name.clone(),
                path: path.clone(),
            };
            // Groups get start/stop/restart all
            let items = vec![
                ContextMenuAction::Start,
                ContextMenuAction::Stop,
                ContextMenuAction::Restart,
            ];
            (target, items, true, Some(format!("Group: {}", name)))
        }
        DockerRow::Item { index, .. } => {
            let container = &containers[*index];
            let target = ContextMenuTarget::Container {
                id: container.id.clone(),
                name: container.name.clone(),
                running: container.running,
            };
            let has_compose_cwd = container
                .group_path
                .as_deref()
                .map(|path| !path.is_empty())
                .unwrap_or(false);
            // Single container - show relevant actions
            let mut items = if container.running {
                vec![
                    ContextMenuAction::Shell,
                    ContextMenuAction::Logs,
                    ContextMenuAction::LogsNewWindow,
                    ContextMenuAction::Env,
                    ContextMenuAction::Stop,
                    ContextMenuAction::Restart,
                ]
            } else {
                vec![
                    ContextMenuAction::Logs,
                    ContextMenuAction::LogsNewWindow,
                    ContextMenuAction::Env,
                    ContextMenuAction::Start,
                ]
            };
            if has_compose_cwd {
            let insert_idx = if container.running { 4 } else { 3 };
            items.insert(insert_idx.min(items.len()), ContextMenuAction::OpenLocation);
            }
            (target, items, false, Some(format!("Container: {}", container.name)))
        }
        DockerRow::Separator => return,
    };

    // Position menu within terminal bounds
    let header_count = if header.is_some() { 1 } else { 0 };
    let (menu_x, menu_y) = position_context_menu(x, y, items.len() + header_count, width, height);

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group,
        header,
    });
}

fn handle_process_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    _main_x: u16,
) {
    // Process view: 3 + 1 + 3 + 4 + 2 = 13
    let list_start: u16 = 13;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        return;
    }

    // Use the same scroll offset as hover handler
    let scroll = state.process_scroll;
    let total = state.visible_pids.len();
    if total == 0 {
        return;
    }

    let target_row = scroll + clicked_visual_row;
    if target_row >= total {
        return;
    }

    let pid = state.visible_pids[target_row];
    let pid_u32 = pid.as_u32();

    // Get process name (use PID as fallback)
    let name = format!("PID {}", pid_u32);

    let target = ContextMenuTarget::Process {
        pid: pid_u32,
        name,
    };

    let items = vec![
        ContextMenuAction::Kill,
        ContextMenuAction::Env,
        ContextMenuAction::Logs,
    ];

    // Position menu within terminal bounds
    let (menu_x, menu_y) = position_context_menu(x, y, items.len(), width, height);

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group: false,
        header: None,
    });
}

fn handle_ports_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    _main_x: u16,
    ports: &[crate::system::ports::PortInfo],
) {
    // Ports view: 3 + 1 + 3 + 2 = 9
    let list_start: u16 = 9;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        return;
    }

    // Use the same scroll offset as hover handler
    let scroll = state.ports_scroll;
    let total = state.visible_ports.len();
    if total == 0 {
        return;
    }

    let target_row = scroll + clicked_visual_row;
    if target_row >= state.visible_ports.len() {
        return;
    }

    // Skip group rows
    if state.is_ports_group_row(target_row) {
        return;
    }

    // Get container_id for this row
    let container_id = state
        .visible_ports_container_ids
        .get(target_row)
        .and_then(|id| id.clone());

    // Find the port info to get the name
    // We need to find the actual port index from the row
    let port_index = find_port_index_for_row(state, target_row, ports);

    let (target, items) = if let Some(container_id) = container_id {
        // Container port - offer Stop action
        let name = port_index
            .map(|idx| ports[idx].name.clone())
            .unwrap_or_else(|| "Container".to_string());

        let target = ContextMenuTarget::Container {
            id: container_id,
            name,
            running: true, // If we see it in ports, it's running
        };

        let items = vec![
            ContextMenuAction::Stop,
            ContextMenuAction::Logs,
            ContextMenuAction::Env,
        ];

        (target, items)
    } else {
        // Regular process port - offer Kill action
        let pid = state.visible_ports[target_row];
        let pid_u32 = pid.as_u32();

        let name = port_index
            .map(|idx| {
                let port = &ports[idx];
                if port.name.is_empty() {
                    format!("PID {}", pid_u32)
                } else {
                    port.name.clone()
                }
            })
            .unwrap_or_else(|| format!("PID {}", pid_u32));

        let target = ContextMenuTarget::Process {
            pid: pid_u32,
            name,
        };

        let items = vec![
            ContextMenuAction::Kill,
            ContextMenuAction::Env,
            ContextMenuAction::Logs,
        ];

        (target, items)
    };

    // Position menu within terminal bounds
    let (menu_x, menu_y) = position_context_menu(x, y, items.len(), width, height);

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group: false,
        header: None,
    });
}

/// Find the index in ports_cache that corresponds to the given visible row
fn find_port_index_for_row(
    state: &AppState,
    target_row: usize,
    ports: &[crate::system::ports::PortInfo],
) -> Option<usize> {
    // Count non-group rows up to target_row to get the item index
    let mut item_count = 0;
    for i in 0..=target_row {
        if !state.is_ports_group_row(i) {
            if i == target_row {
                // This is our target
                return if item_count < ports.len() {
                    Some(item_count)
                } else {
                    None
                };
            }
            item_count += 1;
        }
    }
    None
}

fn handle_node_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    _main_x: u16,
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) {
    if state.pm2_available {
        let (pm2_start, pm2_height, node_start, node_height) = node_table_bounds(height);
        if pm2_height > 0 && y >= pm2_start && y < pm2_start + pm2_height {
            let list_start = pm2_start + 2;
            if y < list_start {
                return;
            }
            let clicked_visual_row = (y - list_start) as usize;
            let visible_height = pm2_height.saturating_sub(2) as usize;
            if visible_height == 0 {
                return;
            }
            let target_row = state.pm2_scroll + clicked_visual_row;
            if target_row >= pm2_rows.len() {
                return;
            }
            let proc = &pm2_view[pm2_rows[target_row]];
            let target = ContextMenuTarget::Pm2 {
                pm_id: proc.pm_id,
                name: proc.name.clone(),
            };
            let status_lower = proc.status.to_lowercase();
            let mut items = Vec::new();
            if status_lower == "online" || status_lower == "launching" || status_lower == "starting" {
                items.push(ContextMenuAction::Stop);
                items.push(ContextMenuAction::Restart);
            } else {
                items.push(ContextMenuAction::Start);
            }
            items.push(ContextMenuAction::Logs);
            if proc.pid.is_some() {
                items.push(ContextMenuAction::Env);
            }
            items.push(ContextMenuAction::OpenLocation);
            let (menu_x, menu_y) = position_context_menu(x, y, items.len(), width, height);
            state.context_menu = Some(ContextMenu {
                x: menu_x,
                y: menu_y,
                items,
                hover: Some(0),
                target,
                is_group: false,
                header: Some(format!("PM2: {}", proc.name)),
            });
            return;
        }
        if node_height == 0 || y < node_start || y >= node_start + node_height {
            return;
        }
    }

    // Node view: 3 + 1 + 3 + 2 = 9
    let list_start: u16 = if state.pm2_available {
        let (_, _, node_start, _) = node_table_bounds(height);
        node_start + 2
    } else {
        9
    };
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_height = 2usize;
    let visible_height = (height as usize).saturating_sub(list_start as usize + footer_height);
    if visible_height == 0 {
        return;
    }

    // Use the same scroll offset as hover handler
    let scroll = state.node_scroll;
    let total = state.visible_pids.len();
    if total == 0 {
        return;
    }

    let target_row = scroll + clicked_visual_row;
    if target_row >= total {
        return;
    }

    // Skip non-selectable rows (spacers, titles, headers)
    if !state.is_node_selectable_row(target_row) {
        return;
    }

    let pid = state.visible_pids[target_row];
    let pid_u32 = pid.as_u32();

    let name = format!("PID {}", pid_u32);

    let target = ContextMenuTarget::Process {
        pid: pid_u32,
        name,
    };

    let items = vec![
        ContextMenuAction::Kill,
        ContextMenuAction::Env,
        ContextMenuAction::Logs,
    ];

    // Position menu within terminal bounds
    let (menu_x, menu_y) = position_context_menu(x, y, items.len(), width, height);

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group: false,
        header: None,
    });
}

fn handle_context_menu_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    containers: &[ContainerInfo],
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) -> Option<bool> {
    let menu = state.context_menu.as_ref()?;
    let x = mouse.column;
    let y = mouse.row;
    let result = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(action) = get_menu_action_at(menu, x, y) {
                let target = menu.target.clone();
                state.context_menu = None;
                execute_context_action(state, action, &target, containers, pm2_view, pm2_rows);
                true
            } else {
                state.context_menu = None;
                true
            }
        }
        MouseEventKind::Moved => {
            let new_hover = get_menu_item_at(menu, x, y);
            if let Some(menu) = state.context_menu.as_mut() {
                if menu.hover != new_hover {
                    menu.hover = new_hover;
                    return Some(true);
                }
            }
            false
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.context_menu = None;
            true
        }
        _ => false,
    };
    Some(result)
}

fn get_menu_item_at(menu: &ContextMenu, x: u16, y: u16) -> Option<usize> {
    let menu_x = menu.x;
    let menu_y = menu.y + MENU_PADDING + menu_header_offset(menu) as u16;
    let menu_width = MENU_WIDTH;

    if x < menu_x || x >= menu_x + menu_width {
        return None;
    }

    if y < menu_y || y >= menu_y + menu.items.len() as u16 {
        return None;
    }

    Some((y - menu_y) as usize)
}

fn get_menu_action_at(menu: &ContextMenu, x: u16, y: u16) -> Option<ContextMenuAction> {
    get_menu_item_at(menu, x, y).map(|idx| menu.items[idx])
}

fn execute_context_action(
    state: &mut AppState,
    action: ContextMenuAction,
    target: &ContextMenuTarget,
    containers: &[ContainerInfo],
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) {
    // Handle process-specific actions
    if let ContextMenuTarget::Process { pid, name } = target {
        match action {
            ContextMenuAction::Kill => {
                use sysinfo::{Pid, Signal, System};

                // Check if this process is managed by PM2
                if crate::system::node::is_pm2_running() {
                    if let Ok(pm2_procs) = crate::system::node::load_pm2_processes() {
                        if let Some(pm2_proc) = pm2_procs.iter().find(|p| p.pid == Some(*pid)) {
                            match crate::system::node::pm2_stop(pm2_proc.pm_id) {
                                Ok(()) => {
                                    state.set_message(format!("Stopped PM2 process {} ({})", pm2_proc.name, name));
                                }
                                Err(err) => {
                                    state.set_message(format!("Failed to stop PM2 process: {}", err));
                                }
                            }
                            return;
                        }
                    }
                }

                let mut sys = System::new();
                sys.refresh_processes();
                let sysinfo_pid = Pid::from_u32(*pid);

                // Check if this process is managed by nodemon/tsx/ts-node-dev
                if let Some(process) = sys.process(sysinfo_pid) {
                    if let Some(parent_pid) = process.parent() {
                        if let Some(parent) = sys.process(parent_pid) {
                            let parent_name = parent.name().to_lowercase();
                            if parent_name.contains("nodemon")
                                || parent_name.contains("tsx")
                                || parent_name.contains("ts-node-dev")
                                || parent_name.contains("node-dev") {
                                // Kill the supervisor parent instead
                                let supervisor_name = parent.name().to_string();
                                let mut killed = parent.kill_with(Signal::Term).unwrap_or(false);
                                if !killed {
                                    state.set_message(format!("Failed to signal {} (PID {})", supervisor_name, parent_pid));
                                    return;
                                }

                                std::thread::sleep(std::time::Duration::from_millis(200));
                                sys.refresh_processes();
                                if sys.process(parent_pid).is_some() {
                                    if let Some(process) = sys.process(parent_pid) {
                                        killed = process.kill_with(Signal::Kill).unwrap_or(false);
                                    }
                                }

                                if killed {
                                    state.set_message(format!("Killed {} (PID {}) and child process", supervisor_name, parent_pid));
                                } else {
                                    state.set_message(format!("Failed to kill {} (PID {})", supervisor_name, parent_pid));
                                }
                                return;
                            }
                        }
                    }
                }

                if let Some(process) = sys.process(sysinfo_pid) {
                    let mut killed = process.kill_with(Signal::Term).unwrap_or(false);
                    if !killed {
                        state.set_message(format!("Failed to signal {}", name));
                        return;
                    }

                    std::thread::sleep(std::time::Duration::from_millis(200));
                    sys.refresh_processes();
                    if sys.process(sysinfo_pid).is_some() {
                        if let Some(process) = sys.process(sysinfo_pid) {
                            killed = process.kill_with(Signal::Kill).unwrap_or(false);
                        }
                    }

                    if killed {
                        state.set_message(format!("Killed {}", name));
                    } else {
                        state.set_message(format!("Failed to kill {}", name));
                    }
                } else {
                    state.set_message(format!("Process {} not found", name));
                }
            }
            ContextMenuAction::Env => {
                let title = "PROCESS ENV";
                enter_env_view(
                    state,
                    state.view_mode,
                    title,
                    format!("Process: {}", name),
                    format!("PID: {}", pid),
                    "-".to_string(),
                    "-".to_string(),
                );
                match process::load_process_env(sysinfo::Pid::from_u32(*pid)) {
                    Ok(envs) => state.env_vars = envs,
                    Err(err) => {
                        state.env_vars = vec![format!("Failed to load env: {err}")];
                    }
                }
            }
            ContextMenuAction::Logs => {
                let pid = *pid;
                let title = format!("Process logs: {}", name);
                start_log_fetch(
                    state,
                    title,
                    LogSource::Process { pid },
                    move || load_process_logs(pid),
                );
            }
            ContextMenuAction::OpenLocation => {
                let path = std::fs::read_link(format!("/proc/{}/cwd", pid));
                match path {
                    Ok(path) => {
                        if let Err(err) = open_path_location(&path) {
                            state.set_message(format!("Failed to open dir: {}", err));
                        } else {
                            state.set_message("Opened process directory.");
                        }
                    }
                    Err(_) => {
                        state.set_message(format!("Failed to read cwd for {}", name));
                    }
                }
            }
            _ => {}
        }
        return;
    }

    if let ContextMenuTarget::Pm2 { pm_id, name } = target {
        match action {
            ContextMenuAction::Logs => {
                let pm_id = *pm_id;
                let title = format!("PM2 logs: {}", name);
                start_log_fetch(
                    state,
                    title,
                    LogSource::Pm2 { pm_id },
                    move || crate::system::node::load_pm2_logs(pm_id),
                );
            }
            ContextMenuAction::Env => {
                if let Some(proc) = pm2_view_for_target(pm2_view, pm2_rows, *pm_id) {
                    open_pm2_env(state, proc);
                } else {
                    state.set_message("PM2 process not found.");
                }
            }
            ContextMenuAction::Start => {
                state.set_message(format!("Starting PM2 {}...", name));
                let _ = crate::system::node::pm2_start(*pm_id);
            }
            ContextMenuAction::Stop => {
                state.set_message(format!("Stopping PM2 {}...", name));
                let _ = crate::system::node::pm2_stop(*pm_id);
            }
            ContextMenuAction::Restart => {
                state.set_message(format!("Restarting PM2 {}...", name));
                let _ = crate::system::node::pm2_restart(*pm_id);
            }
            ContextMenuAction::OpenLocation => {
                if let Some(proc) = pm2_view_for_target(pm2_view, pm2_rows, *pm_id) {
                    open_pm2_location(state, proc);
                }
            }
            _ => {}
        }
        return;
    }

    if matches!(action, ContextMenuAction::Inspect) {
        match target {
            ContextMenuTarget::DockerImage { id, name } => {
                let id = id.clone();
                let title = format!("Inspect image: {}", name);
                start_inspect_fetch(state, title, move || crate::system::docker::inspect_docker_image(&id));
            }
            ContextMenuTarget::DockerContainer { id, name } | ContextMenuTarget::Container { id, name, .. } => {
                let id = id.clone();
                let title = format!("Inspect container: {}", name);
                start_inspect_fetch(state, title, move || crate::system::docker::inspect_docker_container(&id));
            }
            ContextMenuTarget::DockerVolume { name } => {
                let name = name.clone();
                let title = format!("Inspect volume: {}", name);
                start_inspect_fetch(state, title, move || crate::system::docker::inspect_docker_volume(&name));
            }
            _ => {}
        }
        return;
    }

    if matches!(
        action,
        ContextMenuAction::DeleteImage
            | ContextMenuAction::DeleteContainer
            | ContextMenuAction::DeleteVolume
    ) {
        match (action, target) {
            (ContextMenuAction::DeleteImage, ContextMenuTarget::DockerImage { id, name }) => {
                request_delete_confirmation(
                    state,
                    DeleteKind::Image,
                    name.clone(),
                    id.clone(),
                );
            }
            (ContextMenuAction::DeleteContainer, ContextMenuTarget::DockerContainer { id, name }) => {
                request_delete_confirmation(
                    state,
                    DeleteKind::Container,
                    name.clone(),
                    id.clone(),
                );
            }
            (ContextMenuAction::DeleteVolume, ContextMenuTarget::DockerVolume { name }) => {
                request_delete_confirmation(
                    state,
                    DeleteKind::Volume,
                    name.clone(),
                    name.clone(),
                );
            }
            _ => {}
        }
        return;
    }

    if let ContextMenuTarget::Container { id, name, .. } = target {
        if matches!(action, ContextMenuAction::OpenLocation) {
            let compose_path = containers
                .iter()
                .find(|container| container.id == *id)
                .and_then(|container| container.group_path.as_deref());

            if let Some(path) = compose_path {
                if let Err(err) = open_path_location(std::path::Path::new(path)) {
                    state.set_message(format!("Failed to open dir: {}", err));
                } else {
                    state.set_message("Opened compose working directory.");
                }
            } else {
                state.set_message(format!("No compose working directory for {}", name));
            }
            return;
        }
    }

    // Handle container-only actions
    if action.is_container_only() {
        if let ContextMenuTarget::Container { id, name, .. } = target {
            match action {
                ContextMenuAction::Logs => {
                    let title = format!("Docker logs: {}", name);
                    let id = id.clone();
                    start_log_fetch(
                        state,
                        title,
                        LogSource::Docker {
                            container_id: id.clone(),
                        },
                        move || crate::system::docker::load_container_logs(&id),
                    );
                }
                ContextMenuAction::LogsNewWindow => {
                    match crate::system::docker::open_container_logs(id) {
                        Ok(()) => {
                            state.set_message(format!("Opening logs for {}", name));
                        }
                        Err(err) => {
                            state.set_message(format!("Failed to open logs: {}", err));
                        }
                    }
                }
                ContextMenuAction::Shell => {
                    state.set_message(format!("Opening shell in {}...", name));
                    let _ = crate::system::docker::open_container_shell(id);
                }
                ContextMenuAction::Env => {
                    match crate::system::docker::load_container_env(id) {
                        Ok(env_vars) => {
                            state.env_vars = env_vars;
                            enter_env_view(
                                state,
                                ViewMode::Docker,
                                "CONTAINER ENV",
                                format!("Container: {}", name),
                                format!("ID: {}", &id[..12.min(id.len())]),
                                "-".to_string(),
                                "-".to_string(),
                            );
                        }
                        Err(_) => {
                            state.set_message(format!("Failed to load env for {}", name));
                        }
                    }
                }
                _ => {}
            }
        }
        return;
    }

    if matches!(
        action,
        ContextMenuAction::PruneBuildCache
            | ContextMenuAction::PruneDanglingImages
            | ContextMenuAction::PruneVolumes
    ) {
        request_prune_confirmation(state, action);
        return;
    }

    if matches!(
        action,
        ContextMenuAction::ShowImages | ContextMenuAction::ShowContainers | ContextMenuAction::ShowVolumes
    ) {
        match action {
            ContextMenuAction::ShowImages => open_docker_list_modal(state, DockerListKind::Images),
            ContextMenuAction::ShowContainers => open_docker_list_modal(state, DockerListKind::Containers),
            ContextMenuAction::ShowVolumes => open_docker_list_modal(state, DockerListKind::Volumes),
            _ => {}
        }
        return;
    }

    let action_name = match action {
        ContextMenuAction::Start => "Starting",
        ContextMenuAction::Stop => "Stopping",
        ContextMenuAction::Restart => "Restarting",
        _ => return,
    };

    match target {
        ContextMenuTarget::Container { id, name, .. } => {
            state.set_message(format!("{} {}...", action_name, name));
            // Track expected state: Start/Restart -> running, Stop -> stopped
            let expected_running = !matches!(action, ContextMenuAction::Stop);
            state.pending_operations.insert(id.clone(), expected_running);

            let id = id.clone();
            let tx = state.operation_tx.clone();
            std::thread::spawn(move || {
                let result = match action {
                    ContextMenuAction::Start => crate::system::docker::start_container(&id),
                    ContextMenuAction::Stop => crate::system::docker::stop_container(&id),
                    ContextMenuAction::Restart => crate::system::docker::restart_container(&id),
                    _ => Ok(()),
                };
            let _ = tx.send(OperationComplete {
                container_id: id,
                success: result.is_ok(),
                message: result.err().map(|e| e.to_string()).unwrap_or_default(),
                output: None,
            });
        });
        }
        ContextMenuTarget::Pm2 { .. } => {}
        ContextMenuTarget::Group { name, path } => {
            // Find all containers in this group
            let group_containers: Vec<_> = containers
                .iter()
                .filter(|c| c.group_path.as_deref() == path.as_deref())
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();

            if group_containers.is_empty() {
                state.set_message(format!("No containers found in {}", name));
                return;
            }

            let count = group_containers.len();
            state.set_message(format!("{} {} containers in {}...", action_name, count, name));

            // Track expected state: Start/Restart -> running, Stop -> stopped
            let expected_running = !matches!(action, ContextMenuAction::Stop);

            // Mark all containers as pending with expected state
            for (id, _) in &group_containers {
                state.pending_operations.insert(id.clone(), expected_running);
            }

            // Start operations for each container
            for (id, _name) in group_containers {
                let tx = state.operation_tx.clone();
                let container_id = id.clone();
                std::thread::spawn(move || {
                    let result = match action {
                        ContextMenuAction::Start => crate::system::docker::start_container(&container_id),
                        ContextMenuAction::Stop => crate::system::docker::stop_container(&container_id),
                        ContextMenuAction::Restart => crate::system::docker::restart_container(&container_id),
                        _ => Ok(()), // Container-only actions handled earlier
                    };
                    let _ = tx.send(OperationComplete {
                        container_id,
                        success: result.is_ok(),
                        message: result.err().map(|e| e.to_string()).unwrap_or_default(),
                        output: None,
                    });
                });
            }
        }
        // Process targets are handled at the start of the function
        ContextMenuTarget::Process { .. } => {}
        ContextMenuTarget::DockerDf { kind } => {
            let _ = kind;
        }
        ContextMenuTarget::DockerImage { .. } => {}
        ContextMenuTarget::DockerContainer { .. } => {}
        ContextMenuTarget::DockerVolume { .. } => {}
    }
}

fn pm2_view_for_target<'a>(
    pm2_view: &'a [crate::system::node::Pm2Process],
    pm2_rows: &[usize],
    pm_id: u32,
) -> Option<&'a crate::system::node::Pm2Process> {
    pm2_rows
        .iter()
        .filter_map(|idx| pm2_view.get(*idx))
        .find(|proc| proc.pm_id == pm_id)
}

fn open_pm2_env(state: &mut AppState, proc: &crate::system::node::Pm2Process) {
    let pid = proc.pid;
    let script = proc.script.as_deref().unwrap_or("-");
    let cwd = proc.cwd.as_deref().unwrap_or("-");
    let title = format!("PM2 ENV: {}", proc.name);
    enter_env_view(
        state,
        ViewMode::Node,
        &title,
        format!("PM2: {}", proc.name),
        if let Some(pid) = pid {
            format!("PID: {} | PM2: {}", pid, proc.pm_id)
        } else {
            format!("PM2 ID: {}", proc.pm_id)
        },
        format!("Script: {}", script),
        format!("CWD: {}", cwd),
    );

    if let Some(pid) = pid {
        match process::load_process_env(sysinfo::Pid::from_u32(pid)) {
            Ok(envs) => state.env_vars = envs,
            Err(err) => {
                state.env_vars = vec![format!("Failed to load env: {err}")];
            }
        }
    } else {
        match crate::system::node::load_pm2_env(proc.pm_id) {
            Ok(envs) if envs.is_empty() => {
                state.env_vars = vec!["No env data from PM2.".to_string()];
            }
            Ok(envs) => state.env_vars = envs,
            Err(err) => {
                state.env_vars = vec![format!("Failed to load PM2 env: {err}")];
            }
        }
    }
}

fn open_pm2_location(state: &mut AppState, proc: &crate::system::node::Pm2Process) {
    let script = proc.script.as_deref().unwrap_or("");
    let cwd = proc.cwd.as_deref().unwrap_or("");

    let mut target: Option<&std::path::Path> = None;
    if !cwd.is_empty() && cwd != "-" {
        target = Some(std::path::Path::new(cwd));
    } else if !script.is_empty() && script != "-" {
        let path = std::path::Path::new(script);
        if !looks_like_node_binary(path) {
            target = path.parent().or(Some(path));
        }
    }

    if let Some(path) = target {
        if let Err(err) = crate::system::node::open_path_location(path) {
            state.set_message(format!("Failed to open dir: {}", err));
        } else {
            state.set_message("Opened PM2 working directory.");
        }
    } else {
        state.set_message("No working directory for this PM2 process.");
    }
}

fn looks_like_node_binary(path: &std::path::Path) -> bool {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name == "node" || name == "nodejs" || name == "bun" || name == "deno" {
        return true;
    }
    let path_lower = path.to_string_lossy().to_lowercase();
    path_lower.contains("/nvm/")
        || path_lower.contains("/volta/")
        || path_lower.contains("/fnm/")
        || path_lower.ends_with("/bin/node")
        || path_lower.ends_with("/bin/nodejs")
        || path_lower.ends_with("/bin/bun")
        || path_lower.ends_with("/bin/deno")
}

fn open_docker_list_modal(state: &mut AppState, kind: DockerListKind) {
    let (items, label) = match kind {
        DockerListKind::Images => (crate::system::docker::load_docker_images(), "images"),
        DockerListKind::Containers => (
            crate::system::docker::load_docker_containers_with_size(),
            "containers",
        ),
        DockerListKind::Volumes => (crate::system::docker::load_docker_volumes(), "volumes"),
    };

    let mut items = match items {
        Ok(items) => items,
        Err(err) => {
            state.set_message(format!("Failed to load {}: {}", label, err));
            vec![DockerListItem {
                name: format!("Failed to load {}.", label),
                id: "-".to_string(),
                size: "-".to_string(),
                detail_left: "-".to_string(),
                detail_right: "-".to_string(),
            }]
        }
    };

    if matches!(kind, DockerListKind::Images | DockerListKind::Containers) {
        items.sort_by(|a, b| {
            let a_size = docker_size_bytes(&a.size);
            let b_size = docker_size_bytes(&b.size);
            b_size.cmp(&a_size)
        });
    }

    state.context_menu = None;
    state.docker_list_open = true;
    state.docker_list_kind = Some(kind);
    state.docker_list_items = items;
    state.docker_list_selected = 0;
    state.docker_list_hover = false;
}

fn docker_size_bytes(raw: &str) -> u64 {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return 0;
    }
    let main = trimmed
        .split_whitespace()
        .next()
        .unwrap_or(trimmed)
        .split('(')
        .next()
        .unwrap_or(trimmed)
        .trim();
    let mut num = String::new();
    let mut unit = String::new();
    for ch in main.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else if !ch.is_whitespace() {
            unit.push(ch);
        }
    }
    let value: f64 = num.parse().unwrap_or(0.0);
    let unit_lower = unit.to_lowercase();
    let multiplier = match unit_lower.as_str() {
        "b" => 1.0,
        "kb" => 1_000.0,
        "kib" => 1_024.0,
        "mb" => 1_000_000.0,
        "mib" => 1_048_576.0,
        "gb" => 1_000_000_000.0,
        "gib" => 1_073_741_824.0,
        "tb" => 1_000_000_000_000.0,
        "tib" => 1_099_511_627_776.0,
        _ => 1.0,
    };
    (value * multiplier).round() as u64
}

fn request_prune_confirmation(state: &mut AppState, action: ContextMenuAction) {
    if state.pending_prune.is_some() {
        return;
    }
    let label = match action {
        ContextMenuAction::PruneBuildCache => "build cache",
        ContextMenuAction::PruneDanglingImages => "unused images",
        ContextMenuAction::PruneVolumes => "volumes",
        _ => return,
    };
    state.pending_prune = Some(action);
    state.set_message(format!("Confirm prune {}? (y/n)", label));
}

fn request_delete_confirmation(state: &mut AppState, kind: DeleteKind, name: String, id: String) {
    if state.pending_delete.is_some() {
        return;
    }
    state.pending_delete = Some(DeleteConfirm { kind, name, id });
    state.pending_delete_hover = None;
    state.set_message("Confirm delete? (y/n)");
}

fn start_prune_action(state: &mut AppState, action: ContextMenuAction) {
    let (label, command) = match action {
        ContextMenuAction::PruneBuildCache => ("build cache", crate::system::docker::prune_build_cache as fn() -> std::io::Result<String>),
        ContextMenuAction::PruneDanglingImages => ("unused images", crate::system::docker::prune_dangling_images as fn() -> std::io::Result<String>),
        ContextMenuAction::PruneVolumes => ("volumes", crate::system::docker::prune_volumes as fn() -> std::io::Result<String>),
        _ => return,
    };

    state.prune_in_progress = Some(label.to_string());
    state.prune_output = None;
    state.prune_output_hover = false;
    state.set_message(format!("Pruning {}...", label));
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = result.as_ref().ok().cloned();
        let message = match result {
            Ok(output_text) => {
                let formatted = format_prune_output(&output_text);
                if formatted.is_empty() {
                    format!("Pruned {}.", label)
                } else {
                    format!("Pruned {}. {}", label, formatted)
                }
            }
            Err(err) => format!("Failed to prune {}: {}", label, err),
        };
        let _ = tx.send(OperationComplete {
            container_id: format!("prune-{}", label.replace(' ', "-")),
            success,
            message,
            output,
        });
    });
}

fn start_delete_action(state: &mut AppState, confirm: DeleteConfirm) {
    let (kind_label, id_label) = match confirm.kind {
        DeleteKind::Image => ("image", confirm.id.clone()),
        DeleteKind::Container => ("container", confirm.id.clone()),
        DeleteKind::Volume => ("volume", confirm.name.clone()),
    };
    state.set_message(format!("Deleting {} {}...", kind_label, confirm.name));
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = match confirm.kind {
            DeleteKind::Image => crate::system::docker::delete_docker_image(&confirm.id),
            DeleteKind::Container => crate::system::docker::delete_docker_container(&confirm.id),
            DeleteKind::Volume => crate::system::docker::delete_docker_volume(&confirm.name),
        };
        let success = result.is_ok();
        let message = match result {
            Ok(()) => format!("Deleted {} {}", kind_label, confirm.name),
            Err(err) => format!("Failed to delete {} {}: {}", kind_label, confirm.name, err),
        };
        let _ = tx.send(OperationComplete {
            container_id: format!("{}-delete::{}", kind_label, id_label),
            success,
            message,
            output: None,
        });
    });
}

fn menu_header_offset(menu: &ContextMenu) -> usize {
    if menu.header.is_some() { 1 } else { 0 }
}

fn format_prune_output(output: &str) -> String {
    let cleaned = output
        .replace('\r', "")
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let max_len = 220usize;
    if cleaned.len() > max_len {
        format!("Output: {}...", &cleaned[..max_len])
    } else if cleaned.is_empty() {
        String::new()
    } else {
        format!("Output: {}", cleaned)
    }
}

fn handle_prune_confirm_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, yes_area, no_area) =
        prune_confirm_layout(state, main_x, main_width, height);

    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    match mouse.kind {
        MouseEventKind::Moved => {
            if !in_modal {
                if state.pending_prune_hover.is_some() {
                    state.pending_prune_hover = None;
                    return true;
                }
                return false;
            }
            let hover = if point_in_rect(x, y, yes_area) {
                Some(PruneConfirmChoice::Yes)
            } else if point_in_rect(x, y, no_area) {
                Some(PruneConfirmChoice::No)
            } else {
                None
            };
            if state.pending_prune_hover != hover {
                state.pending_prune_hover = hover;
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, yes_area) {
                if let Some(action) = state.pending_prune.take() {
                    state.pending_prune_hover = None;
                    start_prune_action(state, action);
                }
                return true;
            }
            if point_in_rect(x, y, no_area) {
                state.pending_prune = None;
                state.pending_prune_hover = None;
                state.set_message("Prune canceled.");
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.pending_prune = None;
            state.pending_prune_hover = None;
            state.set_message("Prune canceled.");
            true
        }
        _ => in_modal,
    }
}

fn handle_delete_confirm_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, yes_area, no_area) =
        delete_confirm_layout(state, main_x, main_width, height);

    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    match mouse.kind {
        MouseEventKind::Moved => {
            if !in_modal {
                if state.pending_delete_hover.is_some() {
                    state.pending_delete_hover = None;
                    return true;
                }
                return false;
            }
            let hover = if point_in_rect(x, y, yes_area) {
                Some(DeleteConfirmChoice::Yes)
            } else if point_in_rect(x, y, no_area) {
                Some(DeleteConfirmChoice::No)
            } else {
                None
            };
            if state.pending_delete_hover != hover {
                state.pending_delete_hover = hover;
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, yes_area) {
                if let Some(confirm) = state.pending_delete.take() {
                    state.pending_delete_hover = None;
                    start_delete_action(state, confirm);
                }
                return true;
            }
            if point_in_rect(x, y, no_area) {
                state.pending_delete = None;
                state.pending_delete_hover = None;
                state.set_message("Delete canceled.");
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.pending_delete = None;
            state.pending_delete_hover = None;
            state.set_message("Delete canceled.");
            true
        }
        _ => in_modal,
    }
}

fn handle_prune_output_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, close_area) =
        prune_output_layout(state, main_x, main_width, height);

    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    match mouse.kind {
        MouseEventKind::Moved => {
            let hover = point_in_rect(x, y, close_area);
            if state.prune_output_hover != hover {
                state.prune_output_hover = hover;
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, close_area) {
                state.prune_output = None;
                state.prune_output_hover = false;
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.prune_output = None;
            state.prune_output_hover = false;
            true
        }
        _ => in_modal,
    }
}

fn handle_log_output_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, close_area, select_area) =
        log_output_layout(state, main_x, main_width, height);
    let inner_width = modal_w.saturating_sub(4);
    let inner_height = modal_h.saturating_sub(6);
    let show_select = state.log_output_mode == LogOutputMode::Logs;

    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if in_modal && inner_width > 0 && inner_height > 0 {
                return apply_log_scroll(state, -3, inner_width, inner_height);
            }
            in_modal
        }
        MouseEventKind::ScrollDown => {
            if in_modal && inner_width > 0 && inner_height > 0 {
                return apply_log_scroll(state, 3, inner_width, inner_height);
            }
            in_modal
        }
        MouseEventKind::Moved => {
            let close_hover = point_in_rect(x, y, close_area);
            let select_hover = show_select && point_in_rect(x, y, select_area);
            let mut changed = false;
            if state.log_output_hover != close_hover {
                state.log_output_hover = close_hover;
                changed = true;
            }
            if show_select && state.log_select_hover != select_hover {
                state.log_select_hover = select_hover;
                changed = true;
            } else if !show_select && state.log_select_hover {
                state.log_select_hover = false;
                changed = true;
            }
            changed
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, close_area) {
                state.clear_log_state();
                return true;
            }
            if show_select && point_in_rect(x, y, select_area) {
                toggle_log_select_mode(state);
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.clear_log_state();
            true
        }
        _ => in_modal,
    }
}

fn toggle_log_select_mode(state: &mut AppState) {
    state.log_select_mode = !state.log_select_mode;
    state.log_select_hover = false;
    if state.log_select_mode {
        state.log_follow = false;
        state.log_last_scroll = std::time::Instant::now();
    } else if let Some((viewport_w, viewport_h)) = log_modal_inner_size(state) {
        state.log_follow = true;
        state.log_scroll = state.log_max_scroll(viewport_w, viewport_h);
        state.log_last_scroll = std::time::Instant::now();
    }
}

fn handle_env_modal_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, close_area) =
        env_modal_layout(state, main_x, main_width, height);

    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if in_modal && state.env_selected > 0 {
                state.env_selected -= 1;
                return true;
            }
            in_modal
        }
        MouseEventKind::ScrollDown => {
            if in_modal && state.env_selected + 1 < state.env_vars.len() {
                state.env_selected += 1;
                return true;
            }
            in_modal
        }
        MouseEventKind::Moved => {
            let hover = point_in_rect(x, y, close_area);
            if state.env_modal_hover != hover {
                state.env_modal_hover = hover;
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, close_area) {
                state.env_modal_open = false;
                state.env_modal_hover = false;
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.env_modal_open = false;
            state.env_modal_hover = false;
            true
        }
        _ => in_modal,
    }
}

fn handle_docker_list_modal_mouse(
    mouse: MouseEvent,
    state: &mut AppState,
    main_x: u16,
    main_width: u16,
    width: u16,
    height: u16,
) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    let (modal_x, modal_y, modal_w, modal_h, list_area, close_area) =
        docker_list_modal_layout(state, main_x, main_width, height);
    let (list_x, list_y, list_w, list_h) = list_area;
    let in_modal = x >= modal_x
        && x < modal_x + modal_w
        && y >= modal_y
        && y < modal_y + modal_h;

    let total = state.docker_list_items.len();
    let visible = docker_list_visible_height(list_h);
    let scroll = docker_list_scroll_offset(state.docker_list_selected, visible, total);

    let row_at = if y >= list_y.saturating_add(1)
        && y < list_y.saturating_add(list_h)
        && x >= list_x
        && x < list_x + list_w
    {
        let rel = y.saturating_sub(list_y + 1) as usize;
        let idx = scroll.saturating_add(rel);
        if idx < total {
            Some(idx)
        } else {
            None
        }
    } else {
        None
    };

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if in_modal && total > 0 && state.docker_list_selected > 0 {
                state.docker_list_selected -= 1;
                return true;
            }
            in_modal
        }
        MouseEventKind::ScrollDown => {
            if in_modal && total > 0 && state.docker_list_selected + 1 < total {
                state.docker_list_selected += 1;
                return true;
            }
            in_modal
        }
        MouseEventKind::Moved => {
            let hover = point_in_rect(x, y, close_area);
            if state.docker_list_hover != hover {
                state.docker_list_hover = hover;
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(x, y, close_area) {
                state.docker_list_open = false;
                state.docker_list_hover = false;
                state.context_menu = None;
                return true;
            }
            if let Some(idx) = row_at {
                state.docker_list_selected = idx;
                return true;
            }
            in_modal
        }
        MouseEventKind::Down(MouseButton::Right) => {
            if let Some(idx) = row_at {
                state.docker_list_selected = idx;
                if let Some(kind) = state.docker_list_kind {
                    let item = &state.docker_list_items[idx];
                    let (target, header) = match kind {
                        DockerListKind::Images => (
                            ContextMenuTarget::DockerImage {
                                id: item.id.clone(),
                                name: item.name.clone(),
                            },
                            format!("Image: {}", item.name),
                        ),
                        DockerListKind::Containers => (
                            ContextMenuTarget::DockerContainer {
                                id: item.id.clone(),
                                name: item.name.clone(),
                            },
                            format!("Container: {}", item.name),
                        ),
                        DockerListKind::Volumes => (
                            ContextMenuTarget::DockerVolume {
                                name: item.name.clone(),
                            },
                            format!("Volume: {}", item.name),
                        ),
                    };
                    let items = match kind {
        DockerListKind::Images => {
            vec![ContextMenuAction::Inspect, ContextMenuAction::DeleteImage]
        }
        DockerListKind::Containers => {
            vec![ContextMenuAction::Inspect, ContextMenuAction::DeleteContainer]
        }
        DockerListKind::Volumes => {
            vec![
                ContextMenuAction::Inspect,
                ContextMenuAction::DeleteVolume,
                ContextMenuAction::PruneVolumes,
            ]
        }
    };
                    let (menu_x, menu_y) = position_context_menu(x, y, items.len() + 1, width, height);
                    state.context_menu = Some(ContextMenu {
                        x: menu_x,
                        y: menu_y,
                        items,
                        hover: Some(0),
                        target,
                        is_group: false,
                        header: Some(header),
                    });
                    return true;
                }
            }
            state.docker_list_open = false;
            state.docker_list_hover = false;
            state.context_menu = None;
            true
        }
        _ => in_modal,
    }
}

pub(crate) fn log_modal_inner_size(state: &AppState) -> Option<(u16, u16)> {
    const SIDEBAR_WIDTH: u16 = 20;
    const MIN_MAIN_WIDTH: u16 = 40;
    let term_width = state.term_width;
    let term_height = state.term_height;
    if term_width == 0 || term_height == 0 {
        return None;
    }
    let (main_x, main_width) = if term_width >= SIDEBAR_WIDTH + MIN_MAIN_WIDTH {
        (SIDEBAR_WIDTH, term_width - SIDEBAR_WIDTH)
    } else {
        (0, term_width)
    };
    let (_, _, modal_w, modal_h, _, _) = log_output_layout(state, main_x, main_width, term_height);
    let inner_w = modal_w.saturating_sub(4);
    let inner_h = modal_h.saturating_sub(6);
    if inner_w == 0 || inner_h == 0 {
        return None;
    }
    Some((inner_w, inner_h))
}

fn apply_log_scroll(state: &mut AppState, delta: i32, viewport_w: u16, viewport_h: u16) -> bool {
    let max_scroll = state.log_max_scroll(viewport_w, viewport_h);
    if max_scroll == 0 {
        return false;
    }
    let current = if state.log_follow {
        max_scroll
    } else {
        state.log_scroll.min(max_scroll)
    };
    let next = if delta.is_negative() {
        current.saturating_sub(delta.wrapping_abs() as u16)
    } else {
        current.saturating_add(delta as u16).min(max_scroll)
    };
    if next == current {
        return false;
    }
    state.log_scroll = next;
    state.log_follow = next == max_scroll;
    state.log_last_scroll = std::time::Instant::now();
    true
}

fn prune_confirm_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16), (u16, u16, u16, u16)) {
    let label = match state.pending_prune {
        Some(ContextMenuAction::PruneBuildCache) => "build cache",
        Some(ContextMenuAction::PruneDanglingImages) => "dangling images",
        Some(ContextMenuAction::PruneVolumes) => "volumes",
        _ => "",
    };
    let min_width = 54u16;
    let width = min_width.max(label.len() as u16 + 28);
    let height_box = 9u16;
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let buttons_y = y + height_box - 3;
    let button_w = 10u16;
    let button_h = 1u16;
    let gap = 4u16;
    let total_buttons_w = button_w * 2 + gap;
    let buttons_x = x + (width.saturating_sub(total_buttons_w)) / 2;
    let yes_area = (buttons_x, buttons_y, button_w, button_h);
    let no_area = (buttons_x + button_w + gap, buttons_y, button_w, button_h);

    (x, y, width, height_box, yes_area, no_area)
}

fn delete_confirm_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16), (u16, u16, u16, u16)) {
    let label = state
        .pending_delete
        .as_ref()
        .map(|p| p.name.as_str())
        .unwrap_or("");
    let width = 60u16.max(label.len() as u16 + 28);
    let height_box = 9u16;
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let button_w = 10u16;
    let button_h = 1u16;
    let buttons_y = y + height_box - 3;
    let gap = 4u16;
    let total_buttons_w = button_w * 2 + gap;
    let buttons_x = x + (width.saturating_sub(total_buttons_w)) / 2;
    let yes_area = (buttons_x, buttons_y, button_w, button_h);
    let no_area = (buttons_x + button_w + gap, buttons_y, button_w, button_h);

    (x, y, width, height_box, yes_area, no_area)
}

fn prune_output_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16)) {
    let label = state
        .prune_output
        .as_ref()
        .map(|p| p.label.as_str())
        .unwrap_or("");
    let width = (main_width.saturating_mul(85) / 100).max(72).max(label.len() as u16 + 32);
    let height_box = (height.saturating_mul(75) / 100).max(12);
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let button_w = 10u16;
    let button_h = 1u16;
    let button_y = y + height_box - 3;
    let button_x = x + (width.saturating_sub(button_w)) / 2;
    let close_area = (button_x, button_y, button_w, button_h);

    (x, y, width, height_box, close_area)
}

fn log_output_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16), (u16, u16, u16, u16)) {
    let label = state
        .log_output
        .as_ref()
        .map(|p| p.title.as_str())
        .unwrap_or("");
    let max_width = main_width.saturating_sub(2).max(4);
    let max_height = height.saturating_sub(2).max(6);
    let width = (main_width.saturating_mul(92) / 100)
        .max(80)
        .max(label.len() as u16 + 24)
        .min(max_width);
    let height_box = (height.saturating_mul(85) / 100).max(14).min(max_height);
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let button_w = 12u16;
    let button_h = 1u16;
    let button_y = y + height_box - 3;
    let show_select = state.log_output_mode == LogOutputMode::Logs;
    let (close_area, select_area) = if show_select {
        let gap = 4u16;
        let total_w = button_w.saturating_mul(2).saturating_add(gap);
        let button_x = x + (width.saturating_sub(total_w)) / 2;
        let select_area = (button_x, button_y, button_w, button_h);
        let close_area = (button_x + button_w + gap, button_y, button_w, button_h);
        (close_area, select_area)
    } else {
        let button_x = x + (width.saturating_sub(button_w)) / 2;
        let close_area = (button_x, button_y, button_w, button_h);
        let select_area = (0u16, 0u16, 0u16, 0u16);
        (close_area, select_area)
    };

    (x, y, width, height_box, close_area, select_area)
}

fn env_modal_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16)) {
    let label = state.env_title.as_str();
    let max_width = main_width.saturating_sub(2).max(4);
    let max_height = height.saturating_sub(2).max(6);
    let width = (main_width.saturating_mul(90) / 100)
        .max(70)
        .max(label.len() as u16 + 24)
        .min(max_width);
    let height_box = (height.saturating_mul(85) / 100).max(14).min(max_height);
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let button_w = 10u16;
    let button_h = 1u16;
    let button_y = y + height_box - 3;
    let button_x = x + (width.saturating_sub(button_w)) / 2;
    let close_area = (button_x, button_y, button_w, button_h);

    (x, y, width, height_box, close_area)
}

fn docker_list_modal_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (u16, u16, u16, u16, (u16, u16, u16, u16), (u16, u16, u16, u16)) {
    let title = match state.docker_list_kind {
        Some(DockerListKind::Images) => "Docker Images",
        Some(DockerListKind::Containers) => "Docker Containers",
        Some(DockerListKind::Volumes) => "Docker Volumes",
        None => "Docker List",
    };
    let max_width = main_width.saturating_sub(2).max(4);
    let max_height = height.saturating_sub(2).max(6);
    let width = (main_width.saturating_mul(88) / 100)
        .max(70)
        .max(title.len() as u16 + 24)
        .min(max_width);
    let height_box = (height.saturating_mul(80) / 100).max(14).min(max_height);
    let x = main_x + (main_width.saturating_sub(width)) / 2;
    let y = (height.saturating_sub(height_box)) / 2;

    let list_area = (x + 2, y + 2, width.saturating_sub(4), height_box.saturating_sub(6));

    let button_w = 10u16;
    let button_h = 1u16;
    let button_y = y + height_box - 3;
    let button_x = x + (width.saturating_sub(button_w)) / 2;
    let close_area = (button_x, button_y, button_w, button_h);

    (x, y, width, height_box, list_area, close_area)
}

fn docker_list_visible_height(list_height: u16) -> usize {
    list_height.saturating_sub(1) as usize
}

fn docker_list_scroll_offset(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let max_offset = total.saturating_sub(visible);
    let ideal = selected.saturating_sub(visible / 2);
    ideal.min(max_offset)
}

fn point_in_rect(x: u16, y: u16, rect: (u16, u16, u16, u16)) -> bool {
    let (rx, ry, rw, rh) = rect;
    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}
