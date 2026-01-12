use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use sysinfo::System;

use crate::app::actions::{
    kill_selected_in_docker, kill_selected_port_process, kill_selected_process, open_selected_container,
    open_selected_container_logs, open_selected_env,
};
use crate::app::state::{view_for_sidebar_index, ContextMenu, ContextMenuAction, ContextMenuTarget, Focus, InputMode, OperationComplete, PruneConfirmChoice, SortBy, ViewMode};
use crate::app::AppState;
use crate::system::docker::{ContainerInfo, DockerRow};

pub(crate) fn handle_key_event(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
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
    // Close context menu on Escape
    if state.context_menu.is_some() && key.code == KeyCode::Esc {
        state.context_menu = None;
        return false;
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

    if state.pending_prune.is_some() {
        return handle_prune_confirm_mouse(mouse, state, main_x, main_width, height);
    }
    if state.prune_output.is_some() {
        return handle_prune_output_mouse(mouse, state, main_x, main_width, height);
    }

    // If context menu is open, handle it first
    if let Some(ref menu) = state.context_menu {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if click is inside menu
                if let Some(action) = get_menu_action_at(menu, x, y) {
                    let target = menu.target.clone();
                    state.context_menu = None;
                    execute_context_action(state, action, &target, containers);
                    return true;
                }
                // Click outside menu - close it
                state.context_menu = None;
                return true;
            }
            MouseEventKind::Moved => {
                // Update menu hover only if it changed
                let new_hover = get_menu_item_at(menu, x, y);
                if let Some(menu) = state.context_menu.as_mut() {
                    if menu.hover != new_hover {
                        menu.hover = new_hover;
                        return true;
                    }
                }
                return false;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click closes menu
                state.context_menu = None;
                return true;
            }
            _ => return false,
        }
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Clear hover on click
            state.hover_row = None;
            state.sidebar_hover = None;

            if show_sidebar && x < SIDEBAR_WIDTH {
                handle_sidebar_click(state, y);
            } else {
                handle_main_click(state, x.saturating_sub(main_x), y, main_width, height);
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
                    handle_node_right_click(state, x, y, width, height, main_x);
                    true
                }
                _ => false
            }
        }
        MouseEventKind::Moved => {
            // Throttle hover re-renders to avoid excessive CPU usage
            use std::time::Duration;
            const HOVER_RENDER_INTERVAL: Duration = Duration::from_millis(8); // ~60fps max

            if state.last_hover_render.elapsed() < HOVER_RENDER_INTERVAL {
                // Update pending hover row but don't trigger render yet
                if show_sidebar && x < SIDEBAR_WIDTH {
                    state.docker_df_hover = None;
                    handle_sidebar_hover(state, y);
                } else {
                    handle_main_hover(state, x.saturating_sub(main_x), y, height);
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
                state.sidebar_hover = None;
                handle_main_hover(state, x.saturating_sub(main_x), y, height);
                state.hover_row != old_hover || state.docker_df_hover != old_df_hover
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

fn handle_main_click(state: &mut AppState, x: u16, y: u16, width: u16, height: u16) {
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
        ViewMode::Node => 9,
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
            let total = state.visible_pids.len();
            let target_row = state.node_scroll + clicked_visual_row;
            if target_row < total && state.is_node_selectable_row(target_row) {
                state.selected = target_row;
            }
        }
        ViewMode::DockerEnv => {}
    }
}

fn handle_main_hover(state: &mut AppState, _x: u16, y: u16, height: u16) {
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

// Context menu constants
const MENU_WIDTH: u16 = 16;
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
                ContextMenuTarget::DockerDf,
                vec![ContextMenuAction::PruneDanglingImages],
            ),
            3 => (
                ContextMenuTarget::DockerDf,
                vec![ContextMenuAction::PruneBuildCache],
            ),
            2 => (
                ContextMenuTarget::DockerDf,
                vec![ContextMenuAction::PruneVolumes],
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
            // Single container - show relevant actions
            let items = if container.running {
                vec![
                    ContextMenuAction::Shell,
                    ContextMenuAction::Logs,
                    ContextMenuAction::Env,
                    ContextMenuAction::Stop,
                    ContextMenuAction::Restart,
                ]
            } else {
                vec![
                    ContextMenuAction::Logs,
                    ContextMenuAction::Env,
                    ContextMenuAction::Start,
                ]
            };
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
) {
    // Node view: 3 + 1 + 3 + 2 = 9
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
) {
    // Handle process-specific actions
    if let ContextMenuTarget::Process { pid, name } = target {
        match action {
            ContextMenuAction::Kill => {
                use sysinfo::{Pid, Signal, System};
                let mut sys = System::new();
                sys.refresh_processes();
                let sysinfo_pid = Pid::from_u32(*pid);
                if let Some(process) = sys.process(sysinfo_pid) {
                    if process.kill_with(Signal::Term).unwrap_or(false) {
                        state.set_message(format!("Killed {}", name));
                    } else {
                        state.set_message(format!("Failed to kill {}", name));
                    }
                } else {
                    state.set_message(format!("Process {} not found", name));
                }
            }
            ContextMenuAction::Env => {
                use std::fs;
                let env_path = format!("/proc/{}/environ", pid);
                match fs::read(&env_path) {
                    Ok(data) => {
                        let env_vars: Vec<String> = data
                            .split(|&b| b == 0)
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| String::from_utf8(s.to_vec()).ok())
                            .collect();
                        state.env_vars = env_vars;
                        state.env_title = format!("ENV: {}", name);
                        state.env_info_left1 = format!("Process: {}", name);
                        state.env_info_right1 = format!("PID: {}", pid);
                        state.env_info_left2 = "-".to_string();
                        state.env_info_right2 = "-".to_string();
                        state.env_selected = 0;
                        // Return to the view we came from
                        state.env_return_view = state.view_mode;
                        state.view_mode = ViewMode::DockerEnv;
                    }
                    Err(_) => {
                        state.set_message(format!("Failed to read env for {}", name));
                    }
                }
            }
            _ => {}
        }
        return;
    }

    // Handle container-only actions
    if action.is_container_only() {
        if let ContextMenuTarget::Container { id, name, .. } = target {
            match action {
                ContextMenuAction::Logs => {
                    state.set_message(format!("Opening logs for {}...", name));
                    let _ = crate::system::docker::open_container_logs(id);
                }
                ContextMenuAction::Shell => {
                    state.set_message(format!("Opening shell in {}...", name));
                    let _ = crate::system::docker::open_container_shell(id);
                }
                ContextMenuAction::Env => {
                    match crate::system::docker::load_container_env(id) {
                        Ok(env_vars) => {
                            state.env_vars = env_vars;
                            state.env_title = format!("ENV: {}", name);
                            state.env_info_left1 = format!("Container: {}", name);
                            state.env_info_right1 = format!("ID: {}", &id[..12.min(id.len())]);
                            state.env_info_left2 = "-".to_string();
                            state.env_info_right2 = "-".to_string();
                            state.env_selected = 0;
                            state.env_return_view = ViewMode::Docker;
                            state.view_mode = ViewMode::DockerEnv;
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
        ContextMenuTarget::DockerDf => {}
    }
}

fn request_prune_confirmation(state: &mut AppState, action: ContextMenuAction) {
    if state.pending_prune.is_some() {
        return;
    }
    let label = match action {
        ContextMenuAction::PruneBuildCache => "build cache",
        ContextMenuAction::PruneDanglingImages => "dangling images",
        ContextMenuAction::PruneVolumes => "volumes",
        _ => return,
    };
    state.pending_prune = Some(action);
    state.set_message(format!("Confirm prune {}? (y/n)", label));
}

fn start_prune_action(state: &mut AppState, action: ContextMenuAction) {
    let (label, command) = match action {
        ContextMenuAction::PruneBuildCache => ("build cache", crate::system::docker::prune_build_cache as fn() -> std::io::Result<String>),
        ContextMenuAction::PruneDanglingImages => ("dangling images", crate::system::docker::prune_dangling_images as fn() -> std::io::Result<String>),
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

fn point_in_rect(x: u16, y: u16, rect: (u16, u16, u16, u16)) -> bool {
    let (rx, ry, rw, rh) = rect;
    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}
