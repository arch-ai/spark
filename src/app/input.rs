use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind, MouseButton};
use crossterm::terminal;
use sysinfo::System;

use crate::app::actions::{
    kill_selected_in_docker, kill_selected_port_process, kill_selected_process, open_selected_container,
    open_selected_container_logs, open_selected_env,
};
use crate::app::state::{view_for_sidebar_index, Focus, InputMode, SortBy, ViewMode};
use crate::app::AppState;

pub(crate) fn handle_key_event(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if state.view_mode == ViewMode::DockerEnv {
        return handle_env_mode(key, state);
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
        ViewMode::Node => state.visible_pids.len(),
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
const SIDEBAR_MENU_START_ROW: u16 = 10; // After logo, title, separator

pub(crate) fn handle_mouse_event(mouse: MouseEvent, state: &mut AppState) {
    // Only handle left click
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return;
    }

    let (width, height) = terminal::size().unwrap_or((80, 24));
    let x = mouse.column;
    let y = mouse.row;

    // Check if sidebar is visible
    let show_sidebar = width >= SIDEBAR_WIDTH + 1 + 40; // sidebar + gap + min main

    if show_sidebar && x < SIDEBAR_WIDTH {
        // Click in sidebar
        handle_sidebar_click(state, y);
    } else {
        // Click in main area
        let main_x = if show_sidebar { SIDEBAR_WIDTH + 1 } else { 0 };
        handle_main_click(state, x.saturating_sub(main_x), y, height);
    }
}

fn handle_sidebar_click(state: &mut AppState, y: u16) {
    // Menu items start after logo (7 lines), title (1), separator (1) = row 9
    // But there's also top border at row 0, so items are at rows 9, 10, 11, 12
    // Actually looking at the render code: logo takes ~7 rows (1-7), title at 8, sep at 9, items start at 10
    if y < SIDEBAR_MENU_START_ROW {
        return;
    }

    let menu_index = (y - SIDEBAR_MENU_START_ROW) as usize;
    if menu_index < 4 {
        // 4 menu items: Processes, Ports, Docker, Node JS
        state.sidebar_index = menu_index;
        state.set_view(view_for_sidebar_index(menu_index));
        state.focus = Focus::Main;
    }
}

fn handle_main_click(state: &mut AppState, _x: u16, y: u16, _height: u16) {
    // Skip if in filter mode or DockerEnv view
    if state.input_mode == InputMode::Filter {
        return;
    }

    // The list starts at different rows depending on the view
    // Generally: title area + header + system bars + table header
    // For most views, list content starts around row 13-15
    let list_start: u16 = match state.view_mode {
        ViewMode::Process => 13,  // After title, header, cpu/mem/swap bars, table header
        ViewMode::Docker => 13,
        ViewMode::Ports => 13,
        ViewMode::Node => 13,
        ViewMode::DockerEnv => {
            // Env view has its own scroll
            if y >= 6 {
                let clicked_row = (y - 6) as usize;
                if clicked_row < state.env_vars.len() {
                    state.env_selected = clicked_row;
                }
            }
            return;
        }
    };

    if y < list_start {
        return;
    }

    let clicked_row = (y - list_start) as usize;

    match state.view_mode {
        ViewMode::Process => {
            if clicked_row < state.visible_pids.len() {
                state.selected = clicked_row;
            }
        }
        ViewMode::Docker => {
            // Need to account for scroll and find the actual row
            let max_visible = 20usize; // Approximate, depends on terminal height
            let scroll = if state.docker_selected_row >= max_visible {
                state.docker_selected_row.saturating_sub(max_visible - 1)
            } else {
                0
            };
            let target_row = scroll + clicked_row;
            if target_row < state.docker_rows.len() && state.is_docker_selectable_row(target_row) {
                state.docker_selected_row = target_row;
            }
        }
        ViewMode::Ports => {
            let target = clicked_row;
            if target < state.visible_ports.len() && !state.is_ports_group_row(target) {
                state.selected = target;
            }
        }
        ViewMode::Node => {
            if clicked_row < state.visible_pids.len() && state.is_node_selectable_row(clicked_row) {
                state.selected = clicked_row;
            }
        }
        ViewMode::DockerEnv => {}
    }
}
