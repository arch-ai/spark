use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crossterm::terminal;
use sysinfo::System;

use crate::app::actions::{
    kill_selected_in_docker, kill_selected_port_process, kill_selected_process, open_selected_container,
    open_selected_container_logs, open_selected_env,
};
use crate::app::state::{view_for_sidebar_index, ContextMenu, ContextMenuAction, ContextMenuTarget, Focus, InputMode, OperationComplete, SortBy, ViewMode};
use crate::app::AppState;
use crate::system::docker::{ContainerInfo, DockerRow};

pub(crate) fn handle_key_event(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
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

/// Returns true if a re-render is needed
pub(crate) fn handle_mouse_event(mouse: MouseEvent, state: &mut AppState, containers: &[crate::system::docker::ContainerInfo]) -> bool {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let x = mouse.column;
    let y = mouse.row;

    // Check if sidebar is visible
    let show_sidebar = width >= SIDEBAR_WIDTH + 1 + 40; // sidebar + gap + min main

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
                let main_x = if show_sidebar { SIDEBAR_WIDTH + 1 } else { 0 };
                handle_main_click(state, x.saturating_sub(main_x), y, height);
            }
            true
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Right-click to open context menu
            let main_x = if show_sidebar { SIDEBAR_WIDTH + 1 } else { 0 };
            match state.view_mode {
                ViewMode::Docker => {
                    handle_docker_right_click(state, x, y, height, main_x, containers);
                    true
                }
                ViewMode::Process => {
                    handle_process_right_click(state, x, y, height, main_x);
                    true
                }
                ViewMode::Ports => {
                    handle_ports_right_click(state, x, y, height, main_x);
                    true
                }
                _ => false
            }
        }
        MouseEventKind::Moved => {
            // Update hover state
            if show_sidebar && x < SIDEBAR_WIDTH {
                // Hovering over sidebar
                let old_hover = state.sidebar_hover;
                state.hover_row = None;
                handle_sidebar_hover(state, y);
                state.sidebar_hover != old_hover
            } else {
                let old_hover = state.hover_row;
                state.sidebar_hover = None;
                let main_x = if show_sidebar { SIDEBAR_WIDTH + 1 } else { 0 };
                handle_main_hover(state, x.saturating_sub(main_x), y, height);
                state.hover_row != old_hover
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

fn handle_sidebar_hover(state: &mut AppState, y: u16) {
    if y < SIDEBAR_MENU_START_ROW {
        state.sidebar_hover = None;
        return;
    }

    let menu_index = (y - SIDEBAR_MENU_START_ROW) as usize;
    if menu_index < 4 {
        state.sidebar_hover = Some(menu_index);
    } else {
        state.sidebar_hover = None;
    }
}

fn handle_scroll(state: &mut AppState, direction: isize) {
    // Skip if in filter mode
    if state.input_mode == InputMode::Filter {
        return;
    }

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

fn handle_main_click(state: &mut AppState, _x: u16, y: u16, height: u16) {
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

    let clicked_visual_row = (y - list_start) as usize;

    // Calculate max visible rows based on terminal height
    let footer_lines = 5usize;
    let max_rows = (height as usize).saturating_sub(list_start as usize + footer_lines);
    if max_rows == 0 {
        return;
    }

    match state.view_mode {
        ViewMode::Process => {
            // Calculate centered scroll offset
            let total = state.visible_pids.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + clicked_visual_row;
            if target_row < total {
                state.selected = target_row;
            }
        }
        ViewMode::Docker => {
            // Calculate centered scroll for docker view
            let total = state.docker_rows.len();
            let half = max_rows / 2;
            let scroll = if state.docker_selected_row <= half {
                0
            } else if state.docker_selected_row + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.docker_selected_row - half
            };
            let target_row = scroll + clicked_visual_row;
            if target_row < total && state.is_docker_selectable_row(target_row) {
                state.docker_selected_row = target_row;
            }
        }
        ViewMode::Ports => {
            let total = state.visible_ports.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + clicked_visual_row;
            if target_row < total && !state.is_ports_group_row(target_row) {
                state.selected = target_row;
            }
        }
        ViewMode::Node => {
            let total = state.visible_pids.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + clicked_visual_row;
            if target_row < total && state.is_node_selectable_row(target_row) {
                state.selected = target_row;
            }
        }
        ViewMode::DockerEnv => {}
    }
}

fn handle_main_hover(state: &mut AppState, _x: u16, y: u16, height: u16) {
    // Skip if in filter mode
    if state.input_mode == InputMode::Filter {
        state.hover_row = None;
        return;
    }

    let list_start: u16 = match state.view_mode {
        ViewMode::Process => 13,
        ViewMode::Docker => 13,
        ViewMode::Ports => 13,
        ViewMode::Node => 13,
        ViewMode::DockerEnv => {
            if y >= 6 {
                let hover = (y - 6) as usize;
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

    // Calculate max visible rows based on terminal height
    let footer_lines = 5usize;
    let max_rows = (height as usize).saturating_sub(list_start as usize + footer_lines);
    if max_rows == 0 {
        state.hover_row = None;
        return;
    }

    match state.view_mode {
        ViewMode::Process => {
            let total = state.visible_pids.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + hovered_visual_row;
            if target_row < total {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Docker => {
            let total = state.docker_rows.len();
            let half = max_rows / 2;
            let scroll = if state.docker_selected_row <= half {
                0
            } else if state.docker_selected_row + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.docker_selected_row - half
            };
            let target_row = scroll + hovered_visual_row;
            if target_row < total && state.is_docker_selectable_row(target_row) {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Ports => {
            let total = state.visible_ports.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + hovered_visual_row;
            if target_row < total && !state.is_ports_group_row(target_row) {
                state.hover_row = Some(target_row);
            } else {
                state.hover_row = None;
            }
        }
        ViewMode::Node => {
            let total = state.visible_pids.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let target_row = scroll + hovered_visual_row;
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

fn handle_docker_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    height: u16,
    main_x: u16,
    containers: &[crate::system::docker::ContainerInfo],
) {
    let list_start: u16 = 13;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_lines = 5usize;
    let max_rows = (height as usize).saturating_sub(list_start as usize + footer_lines);
    if max_rows == 0 {
        return;
    }

    // Calculate scroll
    let total = state.docker_rows.len();
    let half = max_rows / 2;
    let scroll = if state.docker_selected_row <= half {
        0
    } else if state.docker_selected_row + half >= total {
        total.saturating_sub(max_rows)
    } else {
        state.docker_selected_row - half
    };

    let target_row = scroll + clicked_visual_row;
    if target_row >= state.docker_rows.len() {
        return;
    }

    // Determine target and menu items based on row type
    let (target, items, is_group) = match &state.docker_rows[target_row] {
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
            (target, items, true)
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
            (target, items, false)
        }
        DockerRow::Separator => return,
    };

    // Position menu at click location, adjust if near edges
    let menu_height = items.len() as u16 + MENU_PADDING * 2;
    let menu_x = x.min((main_x + 80).saturating_sub(MENU_WIDTH));
    let menu_y = if y + menu_height >= height {
        y.saturating_sub(menu_height)
    } else {
        y
    };

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group,
    });
}

fn handle_process_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    height: u16,
    main_x: u16,
) {
    let list_start: u16 = 10;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_lines = 5usize;
    let max_rows = (height as usize).saturating_sub(list_start as usize + footer_lines);
    if max_rows == 0 {
        return;
    }

    // Calculate scroll
    let total = state.visible_pids.len();
    if total == 0 {
        return;
    }

    let half = max_rows / 2;
    let scroll = if state.selected <= half {
        0
    } else if state.selected + half >= total {
        total.saturating_sub(max_rows)
    } else {
        state.selected - half
    };

    let target_row = scroll + clicked_visual_row;
    if target_row >= state.visible_pids.len() {
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

    // Position menu at click location, adjust if near edges
    let menu_height = items.len() as u16 + MENU_PADDING * 2;
    let menu_x = x.min((main_x + 80).saturating_sub(MENU_WIDTH));
    let menu_y = if y + menu_height >= height {
        y.saturating_sub(menu_height)
    } else {
        y
    };

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group: false,
    });
}

fn handle_ports_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    height: u16,
    main_x: u16,
) {
    let list_start: u16 = 10;
    if y < list_start {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let footer_lines = 5usize;
    let max_rows = (height as usize).saturating_sub(list_start as usize + footer_lines);
    if max_rows == 0 {
        return;
    }

    // Calculate scroll
    let total = state.visible_ports.len();
    if total == 0 {
        return;
    }

    let half = max_rows / 2;
    let scroll = if state.selected <= half {
        0
    } else if state.selected + half >= total {
        total.saturating_sub(max_rows)
    } else {
        state.selected - half
    };

    let target_row = scroll + clicked_visual_row;
    if target_row >= state.visible_ports.len() {
        return;
    }

    // Skip group rows (pid == 0 and no container)
    if state.is_ports_group_row(target_row) {
        return;
    }

    let pid = state.visible_ports[target_row];
    let pid_u32 = pid.as_u32();

    // Check if this is a container port
    let is_container = state
        .visible_ports_container_ids
        .get(target_row)
        .and_then(|id| id.as_ref())
        .is_some();

    if is_container {
        // For container ports, no action available yet
        return;
    }

    let name = format!("PID {}", pid_u32);

    let target = ContextMenuTarget::Process {
        pid: pid_u32,
        name,
    };

    let items = vec![
        ContextMenuAction::Kill,
        ContextMenuAction::Env,
    ];

    // Position menu at click location, adjust if near edges
    let menu_height = items.len() as u16 + MENU_PADDING * 2;
    let menu_x = x.min((main_x + 80).saturating_sub(MENU_WIDTH));
    let menu_y = if y + menu_height >= height {
        y.saturating_sub(menu_height)
    } else {
        y
    };

    state.context_menu = Some(ContextMenu {
        x: menu_x,
        y: menu_y,
        items,
        hover: Some(0),
        target,
        is_group: false,
    });
}

fn get_menu_item_at(menu: &ContextMenu, x: u16, y: u16) -> Option<usize> {
    let menu_x = menu.x;
    let menu_y = menu.y + MENU_PADDING;
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
                        state.env_return_view = ViewMode::Process;
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
                    });
                });
            }
        }
        // Process targets are handled at the start of the function
        ContextMenuTarget::Process { .. } => {}
    }
}
