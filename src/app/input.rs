use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use sysinfo::System;

use crate::app::actions::{
    enter_env_view, kill_selected_in_docker, kill_selected_port_process, kill_selected_process,
    open_selected_container, open_selected_container_logs, open_selected_env, start_inspect_fetch,
    start_log_fetch,
};
use crate::app::state::{
    view_for_sidebar_index, ContextMenu, ContextMenuAction, ContextMenuTarget, DeleteConfirm,
    DeleteConfirmChoice, DeleteKind, DeleteProgress, DockerDfKind, DockerListKind, Focus,
    InputMode, LogOutputMode, LogSource, NodeTab, OperationComplete, PruneConfirmChoice, SortBy,
    ViewMode,
};
use crate::app::AppState;
use crate::system::docker::{ContainerInfo, DockerRow};
use crate::system::node::open_path_location;
use crate::system::process::load_process_logs;

pub(crate) fn handle_key_event(
    key: KeyEvent,
    state: &mut AppState,
    system: &mut System,
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
    containers: &[ContainerInfo],
    ports: &[crate::system::ports::PortInfo],
) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }
    if let Some(mut menu) = state.sort_menu.take() {
        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Down => {
                menu.selected = (menu.selected + 1).min(menu.target.fields().len() - 1)
            }
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = menu.target.fields().len() - 1,
            KeyCode::Char('r') => {
                let mut sort = state.sort_for(menu.target);
                sort.order = sort.order.toggle();
                state.apply_sort(menu.target, sort);
            }
            KeyCode::Enter => {
                apply_sort_menu(state, menu);
                return false;
            }
            _ => {}
        }
        state.sort_menu = Some(menu);
        return false;
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
    if state.log_output.is_some() {
        if key.code == KeyCode::F(5) {
            let title = state
                .log_output
                .as_ref()
                .map(|o| o.title.clone())
                .unwrap_or_else(|| "Logs".into());
            match state.log_source.clone() {
                Some(LogSource::Process { pid }) => {
                    crate::app::actions::start_log_refresh(state, title, move || {
                        crate::system::process::load_process_logs(pid)
                    })
                }
                Some(LogSource::Pm2 { pm_id }) => {
                    crate::app::actions::start_log_refresh(state, title, move || {
                        crate::system::node::load_pm2_logs(pm_id)
                    })
                }
                Some(LogSource::Docker { container_id }) => {
                    crate::app::actions::start_log_refresh(state, title, move || {
                        crate::system::docker::load_container_logs(&container_id)
                    })
                }
                None => {}
            }
            return false;
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            state.clear_log_state();
        } else if matches!(key.code, KeyCode::Char('v'))
            && state.log_output_mode == LogOutputMode::Logs
        {
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
    if state.log_in_progress.is_some() {
        if key.code == KeyCode::Esc {
            state.clear_log_state();
        }
        return false;
    }
    // The menu owns keyboard input before the list underneath it.
    if let Some(menu) = state.context_menu.as_mut() {
        match key.code {
            KeyCode::Esc => state.context_menu = None,
            KeyCode::Up => menu.hover = Some(menu.hover.unwrap_or(0).saturating_sub(1)),
            KeyCode::Down => {
                menu.hover =
                    Some((menu.hover.unwrap_or(0) + 1).min(menu.items.len().saturating_sub(1)))
            }
            KeyCode::Home => menu.hover = Some(0),
            KeyCode::End => menu.hover = Some(menu.items.len().saturating_sub(1)),
            KeyCode::Enter => {
                let menu = state.context_menu.take().unwrap();
                if let Some(action) = menu.items.get(menu.hover.unwrap_or(0)).copied() {
                    execute_context_action(
                        state,
                        action,
                        &menu.target,
                        containers,
                        pm2_view,
                        pm2_rows,
                    );
                }
            }
            _ => {}
        }
        return false;
    }
    if !state.docker_list_open && !state.env_modal_open {
        if let Some(result) =
            super::workspace_input::key(key, state, containers, ports, pm2_view, pm2_rows)
        {
            return result;
        }
    }
    if key.code == KeyCode::F(10) && state.input_mode == InputMode::Normal && !state.env_modal_open
    {
        open_keyboard_menu(state, containers, ports, pm2_view, pm2_rows);
        return false;
    }
    if state.input_mode == InputMode::Normal && !state.env_modal_open && key.modifiers.is_empty() {
        if key.code == KeyCode::Char('s') {
            state.open_sort_menu();
            return false;
        }
        if key.code == KeyCode::Char('?') {
            open_keyboard_help(state);
            return false;
        }
    }
    if state.docker_list_open {
        return handle_docker_list_modal_mode(key, state);
    }
    if state.env_modal_open {
        return handle_env_modal_mode(key, state);
    }

    if state.view_mode == ViewMode::DockerEnv {
        return handle_env_mode(key, state);
    }

    match state.input_mode {
        InputMode::Normal => handle_normal_mode(key, state, system, pm2_view, pm2_rows),
        InputMode::Filter => handle_filter_mode(key, state),
    }
}

fn open_keyboard_menu(
    state: &mut AppState,
    containers: &[ContainerInfo],
    ports: &[crate::system::ports::PortInfo],
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) {
    let bounds = crate::ui::layout::main_area(ratatui::layout::Rect::new(
        0,
        0,
        state.term_width,
        state.term_height,
    ));
    let x = bounds.x + 2;
    let height = state.term_height;
    let width = state.term_width;
    if state.docker_list_open {
        let Some(item) = state.docker_list_items.get(state.docker_list_selected) else {
            return;
        };
        let (target, delete, label) = match state.docker_list_kind {
            Some(DockerListKind::Images) => (
                ContextMenuTarget::DockerImage {
                    id: item.id.clone(),
                    name: item.name.clone(),
                },
                ContextMenuAction::DeleteImage,
                "Image",
            ),
            Some(DockerListKind::Containers) => (
                ContextMenuTarget::DockerContainer {
                    id: item.id.clone(),
                    name: item.name.clone(),
                },
                ContextMenuAction::DeleteContainer,
                "Container",
            ),
            Some(DockerListKind::Volumes) => (
                ContextMenuTarget::DockerVolume {
                    name: item.name.clone(),
                },
                ContextMenuAction::DeleteVolume,
                "Volume",
            ),
            None => return,
        };
        let mut items = vec![ContextMenuAction::Inspect, delete];
        if state.docker_list_kind == Some(DockerListKind::Volumes) {
            items.push(ContextMenuAction::ShowContainers);
            items.push(ContextMenuAction::PruneVolumes);
        }
        state.context_menu = Some(ContextMenu {
            x,
            y: 3,
            items,
            hover: Some(0),
            target,
            is_group: false,
            header: Some(format!("{label}: {}", item.name)),
        });
        return;
    }
    match state.view_mode {
        ViewMode::Docker => {
            let y = crate::ui::layout::docker_table(bounds.width, height).y
                + 2
                + state
                    .docker_selected_row
                    .saturating_sub(state.docker_scroll) as u16;
            handle_docker_right_click(state, x, y, width, height, bounds.x, containers);
        }
        ViewMode::Process => {
            let y = crate::ui::layout::process_table(bounds.width, height).y
                + 2
                + state.selected.saturating_sub(state.process_scroll) as u16;
            handle_process_right_click(state, x, y, width, height, bounds.x);
        }
        ViewMode::Ports => {
            let y = crate::ui::layout::resource_table(bounds.width, height).y
                + 2
                + state.selected.saturating_sub(state.ports_scroll) as u16;
            handle_ports_right_click(state, x, y, width, height, bounds.x, ports);
        }
        ViewMode::Node => {
            let (pm2, native) = crate::ui::layout::node_tables(bounds, state.node_tab);
            let y = if state.node_tab == NodeTab::Pm2 {
                pm2.y + 2 + state.pm2_selected.saturating_sub(state.pm2_scroll) as u16
            } else {
                native.y + 2 + state.selected.saturating_sub(state.node_scroll) as u16
            };
            handle_node_right_click(state, x, y, width, height, bounds.x, pm2_view, pm2_rows);
        }
        _ => {}
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
        ViewMode::Projects => state.workspace.projects.len(),
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
        } else if state.view_mode == ViewMode::Node && state.node_tab == NodeTab::Pm2 {
            let pm2_idx = state.pm2_selected;
            if let Some(proc) = pm2_rows.get(pm2_idx).and_then(|idx| pm2_view.get(*idx)) {
                match key.code {
                    KeyCode::Char('r') => crate::app::actions::start_pm2_action(
                        state,
                        proc.pm_id,
                        proc.name.clone(),
                        ContextMenuAction::Restart,
                    ),
                    KeyCode::Char('s') => crate::app::actions::start_pm2_action(
                        state,
                        proc.pm_id,
                        proc.name.clone(),
                        ContextMenuAction::Stop,
                    ),
                    KeyCode::Char('t') => crate::app::actions::start_pm2_action(
                        state,
                        proc.pm_id,
                        proc.name.clone(),
                        ContextMenuAction::Start,
                    ),
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
        return false;
    }

    if matches!(key.code, KeyCode::Left | KeyCode::Right) && state.term_width >= 60 {
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
                if state.sidebar_index < 4 {
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

    if state.term_width < 60 {
        state.focus = Focus::Main;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) && state.view_mode == ViewMode::Node {
        state.set_node_tab(match state.node_tab {
            NodeTab::Processes => NodeTab::Pm2,
            NodeTab::Pm2 => NodeTab::Processes,
        });
        return false;
    }
    if state.view_mode == ViewMode::Node && state.node_tab == NodeTab::Pm2 {
        match key.code {
            KeyCode::Up => {
                state.pm2_selected = state.pm2_selected.saturating_sub(1);
                return false;
            }
            KeyCode::Down => {
                state.pm2_selected = (state.pm2_selected + 1).min(pm2_rows.len().saturating_sub(1));
                return false;
            }
            KeyCode::PageUp => {
                state.pm2_selected = state.pm2_selected.saturating_sub(10);
                return false;
            }
            KeyCode::PageDown => {
                state.pm2_selected =
                    (state.pm2_selected + 10).min(pm2_rows.len().saturating_sub(1));
                return false;
            }
            KeyCode::Home => {
                state.pm2_selected = 0;
                return false;
            }
            KeyCode::End => {
                state.pm2_selected = pm2_rows.len().saturating_sub(1);
                return false;
            }
            KeyCode::Char('e') => {
                if let Some(proc) = pm2_rows
                    .get(state.pm2_selected)
                    .and_then(|idx| pm2_view.get(*idx))
                {
                    open_pm2_env(state, proc);
                }
                return false;
            }
            KeyCode::Char('k') => {
                if let Some(proc) = pm2_rows
                    .get(state.pm2_selected)
                    .and_then(|idx| pm2_view.get(*idx))
                {
                    crate::app::actions::start_pm2_action(
                        state,
                        proc.pm_id,
                        proc.name.clone(),
                        ContextMenuAction::Stop,
                    );
                }
                return false;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::F(5) if state.view_mode == ViewMode::Docker => {
            state.docker_refresh_requested = true;
            state.set_message("Refreshing Docker...");
        }
        KeyCode::F(5) => {
            state.refresh_requested = true;
            state.set_message("Refreshing...");
        }
        KeyCode::Char('i') if state.view_mode == ViewMode::Docker => {
            open_docker_list_modal(state, DockerListKind::Images)
        }
        KeyCode::Char('v') if state.view_mode == ViewMode::Docker => {
            open_docker_list_modal(state, DockerListKind::Volumes)
        }
        KeyCode::Char('a') if state.view_mode == ViewMode::Docker => {
            open_docker_list_modal(state, DockerListKind::Containers)
        }
        KeyCode::Char('1') => {
            state.set_view(ViewMode::Process);
            state.focus = Focus::Main;
        }
        KeyCode::Char('2') => {
            state.set_view(ViewMode::Ports);
            state.focus = Focus::Main;
        }
        KeyCode::Char('3') => {
            state.set_view(ViewMode::Docker);
            state.focus = Focus::Main;
        }
        KeyCode::Char('4') => {
            state.set_view(ViewMode::Node);
            state.focus = Focus::Main;
        }
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
            state.reverse_sort();
        }
        KeyCode::Char(' ') => {
            state.logo_animated = !state.logo_animated;
            state.set_message(if state.logo_animated {
                "Logo animation resumed"
            } else {
                "Logo animation paused"
            });
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
            if state.view_mode == ViewMode::Docker {
                state.docker_volume_scope = None;
            }
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
                ViewMode::Node | ViewMode::Projects => ViewMode::Docker,
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
    if key.code == KeyCode::Char('r') {
        state.reverse_sort();
        return false;
    }
    if key.code == KeyCode::Char('c') && state.docker_list_kind == Some(DockerListKind::Volumes) {
        show_volume_containers(state);
        return false;
    }
    if key.code == KeyCode::Delete && state.docker_list_request.is_none() {
        if let (Some(kind), Some(item)) = (
            state.docker_list_kind,
            state
                .docker_list_items
                .get(state.docker_list_selected)
                .cloned(),
        ) {
            let kind = match kind {
                DockerListKind::Images => DeleteKind::Image,
                DockerListKind::Containers => DeleteKind::Container,
                DockerListKind::Volumes => DeleteKind::Volume,
            };
            state.context_menu = None;
            request_delete_confirmation(state, kind, item.name, item.id);
        }
        return false;
    }
    if key.code == KeyCode::Char('i')
        || (key.code == KeyCode::Enter && state.docker_list_kind == Some(DockerListKind::Volumes))
    {
        open_docker_list_details(state);
        return false;
    }
    if key.code == KeyCode::F(5) && state.docker_list_request.is_none() {
        if let Some(kind) = state.docker_list_kind {
            open_docker_list_modal(state, kind);
        }
        return false;
    }
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

fn apply_sort_menu(state: &mut AppState, menu: crate::app::sorting::SortMenu) {
    let field = menu.target.fields()[menu.selected];
    let current = state.sort_for(menu.target);
    let order = if field == current.field {
        current.order.toggle()
    } else {
        field.default_order()
    };
    state.apply_sort(
        menu.target,
        crate::app::sorting::TableSort::new(field, order),
    );
}

fn open_keyboard_help(state: &mut AppState) {
    let fields = state
        .sort_target()
        .fields()
        .iter()
        .map(|field| state.sort_target().field_label(*field))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text=format!("NAVIGATION\n1 Processes   2 Ports   3 Docker   4 Node / PM2   5 Projects\nArrow keys, Home/End and PageUp/PageDown move through rows.\n/ filter   x clear filter   F5 refresh / retry\nF10 or right click opens row actions.\nEsc closes the active dialog. Ctrl+C quits anywhere.\n\nSORTING\nClick a column header to sort; click again to reverse.\nActive headers show ascending ▲ or descending ▼.\ns choose a field   r reverse the active table's order\nAvailable here: {fields}\nSelecting the current sort field reverses its order.\nSort choices are kept separately for each table.\n\nPROCESS / CONTAINER ACTIONS\nk stop / kill   e environment   l logs\nProcesses: c CPU, m tree RAM, n name, z expand / collapse tree\nDocker: Enter shell, i images, v volumes, a container sizes\nDocker cleanup: Ctrl+B cache, Ctrl+I unused images, Ctrl+O anonymous volumes\n\nNODE / PM2\nNode.js Processes is the default tab.\nClick a tab, or use Tab / Shift+Tab to switch to PM2.\nCtrl+R restart, Ctrl+S stop, Ctrl+T start the selected PM2 row.\n\nDISPLAY\nSpace pauses / resumes the ASCII logo animation.\n? opens this help.\n");
    text.push_str("\nWORKSPACE\n5 Projects links processes, PM2, Docker, ports and volumes.\nF2 opens the shared inspector; Tab changes its tab; Esc closes it.\nPorts: Enter selects the owning process or container.\nInspector: l live logs, / search logs, p pause, End follow, y copy path/logs, Y copy command.\nProjects: f favorite, W saved filters, C configure scripts, D dev, P prod.\nStorage: Space selects volumes; Delete reviews exact owners before y confirms cleanup.\nRun: X cancels the running script.\nPreferences are saved in the XDG Spark workspace file.\n");
    if state.docker_list_kind == Some(DockerListKind::Volumes) && state.docker_list_open {
        text.push_str("\nVOLUMES\nEnter / i full details including each container's project directory.\nc show attached containers (x clears this scope in Docker).\nDelete removes attached containers and then the volume after confirmation.\n");
    }
    state.set_log_output("KEYBOARD HELP".into(), text);
    state.log_output_mode = LogOutputMode::Inspect;
    state.log_source = None;
    state.log_follow = false;
}

fn show_volume_containers(state: &mut AppState) {
    let Some(item) = state.docker_list_items.get(state.docker_list_selected) else {
        return;
    };
    let Some(attachments) = &item.attachments else {
        state.set_message("Container references are unavailable. F5 retries the volume list.");
        return;
    };
    let ids = attachments
        .iter()
        .map(|entry| entry.id.clone())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        state.set_message("No known containers are attached to this volume.");
        return;
    }
    let name = item.name.clone();
    state.docker_list_open = false;
    state.context_menu = None;
    state.docker_list_request = None;
    state.docker_filter.clear();
    state.docker_volume_scope = Some((name, ids));
    state.set_view(ViewMode::Docker);
    state.focus = Focus::Main;
    state.docker_refresh_requested = true;
}

fn open_docker_list_details(state: &mut AppState) {
    let Some(item) = state.docker_list_items.get(state.docker_list_selected) else {
        return;
    };
    let target = match state.docker_list_kind {
        Some(DockerListKind::Volumes) => ContextMenuTarget::DockerVolume {
            name: item.name.clone(),
        },
        Some(DockerListKind::Images) => ContextMenuTarget::DockerImage {
            id: item.id.clone(),
            name: item.name.clone(),
        },
        Some(DockerListKind::Containers) => ContextMenuTarget::DockerContainer {
            id: item.id.clone(),
            name: item.name.clone(),
        },
        None => return,
    };
    state.context_menu = None;
    execute_context_action(state, ContextMenuAction::Inspect, &target, &[], &[], &[]);
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
        ViewMode::Projects => "Projects",
    }
}

const SIDEBAR_WIDTH: u16 = 20;
// Sidebar layout: border(1) + logo(7) + spacing(1) = 9

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
    let full_main_area =
        crate::ui::layout::main_area(ratatui::layout::Rect::new(0, 0, width, height));
    let main_area =
        crate::ui::workspace::panes(full_main_area, state.workspace.inspector.is_some()).0;
    let main_x = main_area.x;
    let main_width = main_area.width;
    let show_sidebar = main_x > 0;

    if let Some(mut menu) = state.sort_menu.take() {
        let (area, scroll) =
            crate::ui::layout::sort_menu_area(main_area, menu.target.fields().len(), menu.selected);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if x > area.x && x < area.right() - 1 && y > area.y && y < area.bottom() - 1 {
                    menu.selected = scroll + (y - area.y - 1) as usize;
                    if menu.selected < menu.target.fields().len() {
                        apply_sort_menu(state, menu);
                        return true;
                    }
                }
                return true;
            }
            MouseEventKind::ScrollUp => menu.selected = menu.selected.saturating_sub(1),
            MouseEventKind::ScrollDown => {
                menu.selected = (menu.selected + 1).min(menu.target.fields().len() - 1)
            }
            _ => {}
        }
        state.sort_menu = Some(menu);
        return true;
    }

    if state.pending_delete.is_some() {
        return handle_delete_confirm_mouse(mouse, state, main_x, main_width, height);
    }
    if state.pending_prune.is_some() {
        return handle_prune_confirm_mouse(mouse, state, main_x, main_width, height);
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
    if state.env_modal_open {
        return handle_env_modal_mouse(mouse, state, main_x, main_width, height);
    }
    if state.docker_list_open {
        if let Some(result) =
            handle_context_menu_mouse(mouse, state, containers, pm2_view, pm2_rows, main_area)
        {
            return result;
        }
        if handle_sort_header_click(mouse, state, width, height) {
            return true;
        }
        return handle_docker_list_modal_mouse(mouse, state, main_x, main_width, width, height);
    }

    if let Some(result) =
        handle_context_menu_mouse(mouse, state, containers, pm2_view, pm2_rows, main_area)
    {
        return result;
    }
    if handle_sort_header_click(mouse, state, width, height) {
        return true;
    }

    if let Some(result) = super::workspace_input::mouse(mouse, state, full_main_area) {
        return result;
    }

    if state.view_mode == ViewMode::Node
        && matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        )
    {
        let table = crate::ui::layout::node_layout(main_area)[4];
        if x < main_x
            || x >= main_area.right()
            || y < table.y + 2
            || y >= table.bottom().saturating_sub(1)
        {
            return false;
        }
        if state.node_tab == NodeTab::Pm2 {
            state.pm2_selected = if mouse.kind == MouseEventKind::ScrollUp {
                state.pm2_selected.saturating_sub(1)
            } else {
                (state.pm2_selected + 1).min(pm2_rows.len().saturating_sub(1))
            };
            return true;
        }
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Clear hover on click
            state.hover_row = None;
            state.sidebar_hover = None;

            if show_sidebar && x < SIDEBAR_WIDTH {
                handle_sidebar_click(state, y, height);
            } else {
                handle_main_click(
                    state,
                    x.saturating_sub(main_x),
                    y,
                    main_width,
                    height,
                    pm2_rows,
                );
            }
            true
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Right-click to open context menu
            match state.view_mode {
                ViewMode::Docker => {
                    handle_docker_right_click(
                        state,
                        x,
                        y,
                        main_width + main_x,
                        height,
                        main_x,
                        containers,
                    );
                    true
                }
                ViewMode::Process => {
                    handle_process_right_click(state, x, y, main_width + main_x, height, main_x);
                    true
                }
                ViewMode::Ports => {
                    handle_ports_right_click(
                        state,
                        x,
                        y,
                        main_width + main_x,
                        height,
                        main_x,
                        ports,
                    );
                    true
                }
                ViewMode::Node => {
                    handle_node_right_click(
                        state,
                        x,
                        y,
                        main_width + main_x,
                        height,
                        main_x,
                        pm2_view,
                        pm2_rows,
                    );
                    true
                }
                _ => false,
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
                    handle_sidebar_hover(state, y, height);
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
                handle_sidebar_hover(state, y, height);
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
        _ => false,
    }
}

fn handle_sort_header_click(
    mouse: MouseEvent,
    state: &mut AppState,
    width: u16,
    height: u16,
) -> bool {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return false;
    }
    let hit = {
        let headers = state.rendered_headers.borrow();
        let resource = if state.docker_list_open {
            state.docker_list_kind
        } else {
            None
        };
        if headers.bounds.width != width
            || headers.bounds.height != height
            || headers.view != Some(state.view_mode)
            || headers.resource != resource
            || headers.node_tab != (state.view_mode == ViewMode::Node).then_some(state.node_tab)
        {
            return false;
        }
        headers
            .hits
            .iter()
            .find(|header| {
                header
                    .area
                    .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
            })
            .copied()
    };
    if let Some(hit) = hit {
        state.sort_column(hit.target, hit.field);
        true
    } else {
        false
    }
}

fn handle_sidebar_click(state: &mut AppState, y: u16, height: u16) {
    let menu_start = crate::ui::layout::sidebar_menu_start(height);
    if y < menu_start {
        return;
    }

    let menu_index = (y - menu_start) as usize;
    // 4 menu items: Processes, Ports, Docker, Node JS
    if menu_index < 5 {
        state.sidebar_index = menu_index;
        state.set_view(view_for_sidebar_index(menu_index));
        state.focus = Focus::Main;
    }
}

fn handle_sidebar_hover(state: &mut AppState, y: u16, height: u16) {
    let menu_start = crate::ui::layout::sidebar_menu_start(height);
    if y < menu_start {
        state.sidebar_hover = None;
        return;
    }

    let menu_index = (y - menu_start) as usize;
    // 4 menu items
    if menu_index < 5 {
        state.sidebar_hover = Some(menu_index);
    } else {
        state.sidebar_hover = None;
    }
}

fn jump_to_top(state: &mut AppState) {
    match state.view_mode {
        ViewMode::Projects => {
            state.workspace.selected = 0;
        }
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
        ViewMode::Projects => state.workspace.scroll = 0,
        ViewMode::Process => state.process_scroll = 0,
        ViewMode::Docker => state.docker_scroll = 0,
        ViewMode::Ports => state.ports_scroll = 0,
        ViewMode::Node => state.node_scroll = 0,
        ViewMode::DockerEnv => {}
    }
}

fn jump_to_bottom(state: &mut AppState) {
    match state.view_mode {
        ViewMode::Projects => {
            state.workspace.selected = state.workspace.projects.len().saturating_sub(1)
        }
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
        ViewMode::Projects => {
            state.workspace.selected = state
                .workspace
                .selected
                .saturating_add_signed(direction)
                .min(state.workspace.projects.len().saturating_sub(1));
        }
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

    let area = ratatui::layout::Rect::new(0, 0, width, height);
    if state.view_mode == ViewMode::Node {
        let tabs = crate::ui::layout::node_tab_areas(crate::ui::layout::node_layout(area)[1]);
        if let Some(index) = tabs
            .iter()
            .position(|tab| tab.contains(ratatui::layout::Position::new(x, y)))
        {
            state.input_mode = InputMode::Normal;
            state.set_node_tab(if index == 0 {
                NodeTab::Processes
            } else {
                NodeTab::Pm2
            });
            return;
        }
    }
    let search_area = match state.view_mode {
        ViewMode::Process => crate::ui::layout::process_layout(area)[2],
        ViewMode::Docker => crate::ui::layout::docker_layout(area)[2],
        ViewMode::Node => crate::ui::layout::node_layout(area)[3],
        _ => crate::ui::layout::resource_layout(area)[2],
    };
    let in_search_area = search_area.contains(ratatui::layout::Position::new(x, y))
        && state.view_mode != ViewMode::DockerEnv;

    if in_search_area {
        state.input_mode = InputMode::Filter;
        return;
    } else if state.input_mode == InputMode::Filter {
        // Clicking outside search box exits filter mode
        state.input_mode = InputMode::Normal;
    }

    if state.view_mode == ViewMode::Node {
        let table = crate::ui::layout::node_layout(area)[4];
        if x >= width.saturating_sub(4) {
            if y == table.y {
                if state.node_tab == NodeTab::Pm2 {
                    state.pm2_selected = 0;
                } else {
                    jump_to_top(state);
                }
                return;
            }
            if y == table.bottom().saturating_sub(1) {
                if state.node_tab == NodeTab::Pm2 {
                    state.pm2_selected = pm2_rows.len().saturating_sub(1);
                } else {
                    jump_to_bottom(state);
                }
                return;
            }
        }
        if y >= table.y + 2 && y < table.bottom().saturating_sub(1) {
            let offset = (y - table.y - 2) as usize;
            if state.node_tab == NodeTab::Pm2 {
                let row = state.pm2_scroll + offset;
                if row < pm2_rows.len() {
                    state.pm2_selected = row;
                }
            } else {
                let row = state.node_scroll + offset;
                if row < state.visible_pids.len() && state.is_node_selectable_row(row) {
                    state.selected = row;
                }
            }
        }
        return;
    }

    // Row offsets for ratatui layout:
    // Process: 3 (title) + 1 (header) + 3 (search) + 4 (bars) + 2 (table border+header) = 13
    // Docker: 3 (title) + 1 (header) + 3 (search) + 7 (df stats) + 2 (table border+header) = 16
    // Ports/Node: 3 + 1 + 3 + 2 = 9
    let list_start: u16 = match state.view_mode {
        ViewMode::Process => crate::ui::layout::process_table(width, height).y + 2,
        ViewMode::Docker => crate::ui::layout::docker_table(width, height).y + 2,
        ViewMode::Ports => crate::ui::layout::resource_table(width, height).y + 2,
        ViewMode::Node | ViewMode::Projects => return,
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

    if state.view_mode == ViewMode::Process
        && y >= crate::ui::layout::process_table(width, height)
            .bottom()
            .saturating_sub(1)
    {
        state.hover_row = None;
        return;
    }
    if state.view_mode == ViewMode::Docker
        && y >= crate::ui::layout::docker_table(width, height)
            .bottom()
            .saturating_sub(1)
    {
        state.hover_row = None;
        return;
    }
    if y < list_start {
        return;
    }

    let bottom = match state.view_mode {
        ViewMode::Process => crate::ui::layout::process_table(width, height).bottom(),
        ViewMode::Docker => crate::ui::layout::docker_table(width, height).bottom(),
        _ => crate::ui::layout::resource_table(width, height).bottom(),
    };
    if y >= bottom.saturating_sub(1) {
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
        ViewMode::Node | ViewMode::Projects => {}
        ViewMode::DockerEnv => {}
    }
}

fn handle_main_hover(state: &mut AppState, _x: u16, y: u16, height: u16, pm2_rows: &[usize]) {
    let main_width = crate::ui::workspace::panes(
        crate::ui::layout::main_area(ratatui::layout::Rect::new(0, 0, state.term_width, height)),
        state.workspace.inspector.is_some(),
    )
    .0
    .width;
    // Docker df stats area: title(3) + header(1) + search(3) = 7, df stats is 7 rows
    // Data rows start at row 9 (0-indexed: 9, 10, 11, 12 for Images, Containers, Volumes, Build Cache)
    if let Some(df_hover) = (state.view_mode == ViewMode::Docker)
        .then(|| crate::ui::layout::docker_disk_row(main_width, height, y))
        .flatten()
    {
        if df_hover < 4 {
            state.docker_df_hover = Some(df_hover);
            state.hover_row = None;
            return;
        }
    } else if state.docker_df_hover.is_some() {
        state.docker_df_hover = None;
    }

    if state.view_mode != ViewMode::Node || state.node_tab != NodeTab::Pm2 {
        state.pm2_hover_row = None;
    }

    if state.view_mode == ViewMode::Node {
        let (pm2_start, pm2_height, node_start, node_height) =
            node_table_bounds(state, main_width, height);
        if pm2_height > 0 && y >= pm2_start && y < pm2_start + pm2_height.saturating_sub(1) {
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
        ViewMode::Process => crate::ui::layout::process_table(main_width, height).y + 2,
        ViewMode::Docker => crate::ui::layout::docker_table(main_width, height).y + 2,
        ViewMode::Ports => crate::ui::layout::resource_table(main_width, height).y + 2,
        ViewMode::Node | ViewMode::Projects => return,
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

    if state.view_mode == ViewMode::Process
        && y >= crate::ui::layout::process_table(main_width, height)
            .bottom()
            .saturating_sub(1)
    {
        state.hover_row = None;
        return;
    }
    if state.view_mode == ViewMode::Docker
        && y >= crate::ui::layout::docker_table(main_width, height)
            .bottom()
            .saturating_sub(1)
    {
        state.hover_row = None;
        return;
    }
    let bottom = match state.view_mode {
        ViewMode::Process => crate::ui::layout::process_table(main_width, height).bottom(),
        ViewMode::Docker => crate::ui::layout::docker_table(main_width, height).bottom(),
        _ => crate::ui::layout::resource_table(main_width, height).bottom(),
    };
    if y >= bottom.saturating_sub(1) {
        state.hover_row = None;
        return;
    }
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
        ViewMode::Node | ViewMode::Projects => {}
        ViewMode::DockerEnv => {}
    }
}

fn node_table_bounds(state: &AppState, width: u16, height: u16) -> (u16, u16, u16, u16) {
    let (pm2, native) = crate::ui::layout::node_tables(
        ratatui::layout::Rect::new(0, 0, width, height),
        state.node_tab,
    );
    (pm2.y, pm2.height, native.y, native.height)
}

fn handle_pm2_hover(
    state: &mut AppState,
    y: u16,
    table_start: u16,
    table_height: u16,
    pm2_rows: &[usize],
) {
    if y >= table_start + table_height.saturating_sub(1) {
        state.pm2_hover_row = None;
        return;
    }
    let list_start = table_start + 2;
    let visible_height = table_height.saturating_sub(3) as usize;
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
    if y >= table_start + table_height.saturating_sub(1) {
        state.hover_row = None;
        return;
    }
    let list_start = table_start + 2;
    if y < list_start {
        state.hover_row = None;
        return;
    }
    let hovered_visual_row = (y - list_start) as usize;
    let visible_height = table_height.saturating_sub(3) as usize;
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
    main_x: u16,
    containers: &[crate::system::docker::ContainerInfo],
) {
    if x < main_x {
        return;
    }
    if let Some(df_hover) =
        crate::ui::layout::docker_disk_row(width.saturating_sub(main_x), height, y)
    {
        let (target, items) = match df_hover {
            0 => (
                ContextMenuTarget::DockerDf {
                    kind: DockerDfKind::Images,
                },
                vec![
                    ContextMenuAction::ShowImages,
                    ContextMenuAction::PruneDanglingImages,
                ],
            ),
            1 => (
                ContextMenuTarget::DockerDf {
                    kind: DockerDfKind::Containers,
                },
                vec![ContextMenuAction::ShowContainers],
            ),
            3 => (
                ContextMenuTarget::DockerDf {
                    kind: DockerDfKind::BuildCache,
                },
                vec![ContextMenuAction::PruneBuildCache],
            ),
            2 => (
                ContextMenuTarget::DockerDf {
                    kind: DockerDfKind::Volumes,
                },
                vec![
                    ContextMenuAction::ShowVolumes,
                    ContextMenuAction::PruneVolumes,
                ],
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
    let list_start = crate::ui::layout::docker_table(width.saturating_sub(main_x), height).y + 2;
    if state.view_mode == ViewMode::Docker
        && y >= crate::ui::layout::docker_table(width.saturating_sub(main_x), height)
            .bottom()
            .saturating_sub(1)
    {
        state.hover_row = None;
        return;
    }
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
    state.docker_selected_row = target_row;

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
            (
                target,
                items,
                false,
                Some(format!("Container: {}", container.name)),
            )
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
    main_x: u16,
) {
    if x < main_x {
        return;
    }

    let table = crate::ui::layout::process_table(width.saturating_sub(main_x), height);
    let list_start = table.y + 2;
    if y < list_start || y >= table.bottom().saturating_sub(1) {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let visible_height = table.height.saturating_sub(3) as usize;
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

    state.selected = target_row;
    let pid = state.visible_pids[target_row];
    let pid_u32 = pid.as_u32();

    // Get process name (use PID as fallback)
    let name = format!("PID {}", pid_u32);

    let target = ContextMenuTarget::Process { pid: pid_u32, name };

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
    main_x: u16,
    ports: &[crate::system::ports::PortInfo],
) {
    if x < main_x {
        return;
    }

    let table = crate::ui::layout::resource_table(width.saturating_sub(main_x), height);
    let list_start = table.y + 2;
    if y < list_start || y >= table.bottom().saturating_sub(1) {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let visible_height = table.height.saturating_sub(3) as usize;
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

    state.selected = target_row;
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

        if pid_u32 == 0 {
            state.set_message("The listener owner is unavailable or inaccessible.");
            return;
        }
        let target = ContextMenuTarget::Process { pid: pid_u32, name };

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
    state
        .visible_port_indices
        .get(target_row)
        .copied()
        .flatten()
        .filter(|index| *index < ports.len())
}

fn handle_node_right_click(
    state: &mut AppState,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    main_x: u16,
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) {
    if x < main_x {
        return;
    }

    if state.node_tab == NodeTab::Pm2 {
        let (pm2_start, pm2_height, _, _) =
            node_table_bounds(state, width.saturating_sub(main_x), height);
        if pm2_height > 0 && y >= pm2_start && y < pm2_start + pm2_height.saturating_sub(1) {
            let list_start = pm2_start + 2;
            if y < list_start {
                return;
            }
            let clicked_visual_row = (y - list_start) as usize;
            let visible_height = pm2_height.saturating_sub(3) as usize;
            if visible_height == 0 {
                return;
            }
            let target_row = state.pm2_scroll + clicked_visual_row;
            if target_row >= pm2_rows.len() {
                return;
            }
            state.pm2_selected = target_row;
            let proc = &pm2_view[pm2_rows[target_row]];
            let target = ContextMenuTarget::Pm2 {
                pm_id: proc.pm_id,
                name: proc.name.clone(),
            };
            let status_lower = proc.status.to_lowercase();
            let mut items = Vec::new();
            if status_lower == "online" || status_lower == "launching" || status_lower == "starting"
            {
                items.push(ContextMenuAction::Stop);
                items.push(ContextMenuAction::Restart);
            } else {
                items.push(ContextMenuAction::Start);
            }
            items.push(ContextMenuAction::Logs);
            items.push(ContextMenuAction::Env);
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
        return;
    }

    let (_, _, node_start, node_height) =
        node_table_bounds(state, width.saturating_sub(main_x), height);
    let list_start = node_start + 2;
    if y < list_start || y >= node_start + node_height.saturating_sub(1) {
        return;
    }

    let clicked_visual_row = (y - list_start) as usize;
    let visible_height = node_height.saturating_sub(3) as usize;
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

    state.selected = target_row;

    let name = format!("PID {}", pid_u32);

    let target = ContextMenuTarget::Process { pid: pid_u32, name };

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
    bounds: ratatui::layout::Rect,
) -> Option<bool> {
    let menu = state.context_menu.as_ref()?;
    let x = mouse.column;
    let y = mouse.row;
    let result = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(action) = get_menu_action_at(menu, x, y, bounds) {
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
            let new_hover = get_menu_item_at(menu, x, y, bounds);
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

fn get_menu_item_at(
    menu: &ContextMenu,
    x: u16,
    y: u16,
    bounds: ratatui::layout::Rect,
) -> Option<usize> {
    let labels: Vec<_> = menu
        .items
        .iter()
        .map(|action| action.label(menu.is_group))
        .collect();
    let area = crate::ui::layout::context_menu_area(
        bounds,
        menu.x,
        menu.y,
        &labels,
        menu.header.as_deref(),
    );
    let menu_y = area.y + MENU_PADDING + menu_header_offset(menu) as u16;
    if x <= area.x || x >= area.right().saturating_sub(1) {
        return None;
    }

    if y < menu_y || y >= menu_y + menu.items.len() as u16 || y >= area.bottom().saturating_sub(1) {
        return None;
    }

    let capacity = area
        .height
        .saturating_sub(2 + menu_header_offset(menu) as u16) as usize;
    let scroll = menu
        .hover
        .unwrap_or(0)
        .saturating_sub(capacity.saturating_sub(1));
    Some(scroll + (y - menu_y) as usize).filter(|idx| *idx < menu.items.len())
}

fn get_menu_action_at(
    menu: &ContextMenu,
    x: u16,
    y: u16,
    bounds: ratatui::layout::Rect,
) -> Option<ContextMenuAction> {
    get_menu_item_at(menu, x, y, bounds).map(|idx| menu.items[idx])
}

fn execute_context_action(
    state: &mut AppState,
    action: ContextMenuAction,
    target: &ContextMenuTarget,
    containers: &[ContainerInfo],
    pm2_view: &[crate::system::node::Pm2Process],
    pm2_rows: &[usize],
) {
    if let ContextMenuTarget::DockerVolume { name } = target {
        if action == ContextMenuAction::ShowContainers {
            if let Some(index) = state
                .docker_list_items
                .iter()
                .position(|item| &item.name == name)
            {
                state.docker_list_selected = index;
                show_volume_containers(state);
            }
            return;
        }
    }
    // Handle process-specific actions
    if let ContextMenuTarget::Process { pid, name } = target {
        match action {
            ContextMenuAction::Kill => crate::app::actions::start_process_kill(state, *pid),
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
                crate::app::actions::start_process_env_fetch(state, sysinfo::Pid::from_u32(*pid));
            }
            ContextMenuAction::Logs => {
                let pid = *pid;
                let title = format!("Process logs: {}", name);
                start_log_fetch(state, title, LogSource::Process { pid }, move || {
                    load_process_logs(pid)
                });
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
                start_log_fetch(state, title, LogSource::Pm2 { pm_id }, move || {
                    crate::system::node::load_pm2_logs(pm_id)
                });
            }
            ContextMenuAction::Env => {
                if let Some(proc) = pm2_view_for_target(pm2_view, pm2_rows, *pm_id) {
                    open_pm2_env(state, proc);
                } else {
                    state.set_message("PM2 process not found.");
                }
            }
            ContextMenuAction::Start | ContextMenuAction::Stop | ContextMenuAction::Restart => {
                crate::app::actions::start_pm2_action(state, *pm_id, name.clone(), action);
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
                start_inspect_fetch(state, title, move || {
                    crate::system::docker::inspect_docker_image(&id)
                });
            }
            ContextMenuTarget::DockerContainer { id, name }
            | ContextMenuTarget::Container { id, name, .. } => {
                let id = id.clone();
                let title = format!("Inspect container: {}", name);
                start_inspect_fetch(state, title, move || {
                    crate::system::docker::inspect_docker_container(&id)
                });
            }
            ContextMenuTarget::DockerVolume { name } => {
                let name = name.clone();
                let title = format!("Volume details: {}", name);
                start_inspect_fetch(state, title, move || {
                    crate::system::docker::inspect_docker_volume(&name)
                });
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
                request_delete_confirmation(state, DeleteKind::Image, name.clone(), id.clone());
            }
            (
                ContextMenuAction::DeleteContainer,
                ContextMenuTarget::DockerContainer { id, name },
            ) => {
                request_delete_confirmation(state, DeleteKind::Container, name.clone(), id.clone());
            }
            (ContextMenuAction::DeleteVolume, ContextMenuTarget::DockerVolume { name }) => {
                request_delete_confirmation(state, DeleteKind::Volume, name.clone(), name.clone());
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
                ContextMenuAction::Shell => match crate::system::docker::open_container_shell(id) {
                    Ok(()) => state.set_message(format!("Opening shell in {name}...")),
                    Err(err) => state.set_message(format!("Failed to open terminal: {err}")),
                },
                ContextMenuAction::Env => {
                    enter_env_view(
                        state,
                        ViewMode::Docker,
                        "CONTAINER ENV",
                        format!("Container: {name}"),
                        format!("ID: {id}"),
                        "-".to_string(),
                        "-".to_string(),
                    );
                    crate::app::actions::start_container_env_fetch(state, id.clone());
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
        ContextMenuAction::ShowImages
            | ContextMenuAction::ShowContainers
            | ContextMenuAction::ShowVolumes
    ) {
        match action {
            ContextMenuAction::ShowImages => open_docker_list_modal(state, DockerListKind::Images),
            ContextMenuAction::ShowContainers => {
                open_docker_list_modal(state, DockerListKind::Containers)
            }
            ContextMenuAction::ShowVolumes => {
                open_docker_list_modal(state, DockerListKind::Volumes)
            }
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
            if state.pending_operations.contains_key(id) {
                return;
            }
            state.set_message(format!("{} {}...", action_name, name));
            // Track expected state: Start/Restart -> running, Stop -> stopped
            let expected_running = !matches!(action, ContextMenuAction::Stop);
            state
                .pending_operations
                .insert(id.clone(), expected_running);

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
                    request_id: None,
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
                .filter(|c| crate::system::docker::matches_group(c, name, path.as_deref()))
                .filter(|c| !state.pending_operations.contains_key(&c.id))
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();

            if group_containers.is_empty() {
                state.set_message(format!("No containers found in {}", name));
                return;
            }

            let count = group_containers.len();
            state.set_message(format!(
                "{} {} containers in {}...",
                action_name, count, name
            ));

            // Track expected state: Start/Restart -> running, Stop -> stopped
            let expected_running = !matches!(action, ContextMenuAction::Stop);

            // Mark all containers as pending with expected state
            for (id, _) in &group_containers {
                state
                    .pending_operations
                    .insert(id.clone(), expected_running);
            }

            let tx = state.operation_tx.clone();
            std::thread::spawn(move || {
                for (container_id, name) in group_containers {
                    let result = match action {
                        ContextMenuAction::Start => {
                            crate::system::docker::start_container(&container_id)
                        }
                        ContextMenuAction::Stop => {
                            crate::system::docker::stop_container(&container_id)
                        }
                        ContextMenuAction::Restart => {
                            crate::system::docker::restart_container(&container_id)
                        }
                        _ => Ok(()),
                    };
                    let success = result.is_ok();
                    let message = result
                        .err()
                        .map(|e| format!("{name}: {e}"))
                        .unwrap_or_else(|| format!("{name}: action completed"));
                    let _ = tx.send(OperationComplete {
                        request_id: None,
                        container_id,
                        success,
                        message,
                        output: None,
                    });
                }
            });
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
    let title = format!("PM2 ENV: {}", proc.name);
    enter_env_view(
        state,
        ViewMode::Node,
        &title,
        format!("PM2: {}", proc.name),
        format!(
            "PM2 ID: {} | PID: {}",
            proc.pm_id,
            proc.pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".into())
        ),
        format!("Script: {}", proc.script.as_deref().unwrap_or("-")),
        format!("CWD: {}", proc.cwd.as_deref().unwrap_or("-")),
    );
    if let Some(pid) = proc.pid {
        crate::app::actions::start_process_env_fetch(state, sysinfo::Pid::from_u32(pid));
    } else {
        let pm_id = proc.pm_id;
        crate::app::actions::start_env_fetch(state, move || {
            crate::system::node::load_pm2_env(pm_id).map_err(std::io::Error::other)
        });
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
    let (tx, rx) = std::sync::mpsc::channel();
    state.docker_list_restore = if state.docker_list_open && state.docker_list_kind == Some(kind) {
        state
            .docker_list_items
            .get(state.docker_list_selected)
            .map(|item| (item.id.clone(), item.name.clone()))
    } else {
        None
    };
    state.context_menu = None;
    state.docker_list_open = true;
    state.docker_list_kind = Some(kind);
    state.docker_list_items.clear();
    state.docker_list_selected = 0;
    state.docker_list_hover = false;
    state.docker_list_error = None;
    state.docker_list_request = Some(rx);
    std::thread::spawn(move || {
        let result = match kind {
            DockerListKind::Images => crate::system::docker::load_docker_images(),
            DockerListKind::Containers => crate::system::docker::load_docker_containers_with_size(),
            DockerListKind::Volumes => crate::system::docker::load_docker_volumes(),
        };
        let _ = tx.send(result);
    });
}

#[cfg(test)]
fn docker_size_bytes(raw: &str) -> u64 {
    crate::app::sorting::size_bytes(raw).unwrap_or(0)
}

fn request_prune_confirmation(state: &mut AppState, action: ContextMenuAction) {
    if state.pending_prune.is_some()
        || state.prune_in_progress.is_some()
        || state.pending_delete.is_some()
        || state.delete_in_progress.is_some()
        || state.mutation_result.is_some()
    {
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
    if state.pending_delete.is_some()
        || state.delete_in_progress.is_some()
        || state.mutation_result.is_some()
        || state.pending_prune.is_some()
        || state.prune_in_progress.is_some()
    {
        return;
    }
    state.pending_delete = Some(DeleteConfirm { kind, name, id });
    state.pending_delete_hover = None;
    state.set_message("Confirm delete? (y/n)");
}

fn start_prune_action(state: &mut AppState, action: ContextMenuAction) {
    let (label, command) = match action {
        ContextMenuAction::PruneBuildCache => (
            "build cache",
            crate::system::docker::prune_build_cache as fn() -> std::io::Result<String>,
        ),
        ContextMenuAction::PruneDanglingImages => (
            "unused images",
            crate::system::docker::prune_dangling_images as fn() -> std::io::Result<String>,
        ),
        ContextMenuAction::PruneVolumes => (
            "volumes",
            crate::system::docker::prune_volumes as fn() -> std::io::Result<String>,
        ),
        _ => return,
    };

    state.prune_in_progress = Some(label.to_string());
    state.set_message(format!("Pruning {}...", label));
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = command();
        let success = result.is_ok();
        let output = result.as_ref().ok().cloned();
        let message = match result {
            Ok(output_text) => format!("Pruned {label}.\n\n{output_text}"),
            Err(err) => format!("Failed to prune {label}:\n\n{err}"),
        };
        let _ = tx.send(OperationComplete {
            request_id: None,
            container_id: format!("prune-{}", label.replace(' ', "-")),
            success,
            message,
            output,
        });
    });
}

fn start_delete_action(state: &mut AppState, confirm: DeleteConfirm) {
    if state.workspace.cleanup_busy() {
        state.set_message("Wait for the project cleanup to finish.");
        return;
    }
    if state.delete_in_progress.is_some() || state.mutation_result.is_some() {
        return;
    }
    let (kind_label, id_label) = match confirm.kind {
        DeleteKind::Image => ("image", confirm.id.clone()),
        DeleteKind::Container => ("container", confirm.id.clone()),
        DeleteKind::Volume => ("volume", confirm.name.clone()),
    };
    state.delete_in_progress = Some(DeleteProgress {
        label: format!("{} {}", kind_label, confirm.name),
        started_at: std::time::Instant::now(),
    });
    let tx = state.operation_tx.clone();
    std::thread::spawn(move || {
        let result = match confirm.kind {
            DeleteKind::Image => {
                crate::system::docker::delete_docker_image(&confirm.id).map(|()| String::new())
            }
            DeleteKind::Container => {
                crate::system::docker::delete_docker_container(&confirm.id).map(|()| String::new())
            }
            DeleteKind::Volume => crate::system::docker::delete_docker_volume(&confirm.name),
        };
        let success = result.is_ok();
        let message = match result {
            Ok(details) => format!("Deleted {} {}\n\n{}", kind_label, confirm.name, details),
            Err(err) => format!("Failed to delete {} {}: {}", kind_label, confirm.name, err),
        };
        let _ = tx.send(OperationComplete {
            request_id: None,
            container_id: format!("{}-delete::{}", kind_label, id_label),
            success,
            message,
            output: None,
        });
    });
}

fn menu_header_offset(menu: &ContextMenu) -> usize {
    if menu.header.is_some() {
        1
    } else {
        0
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

    let in_modal = x >= modal_x && x < modal_x + modal_w && y >= modal_y && y < modal_y + modal_h;

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

    let in_modal = x >= modal_x && x < modal_x + modal_w && y >= modal_y && y < modal_y + modal_h;

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

    let in_modal = x >= modal_x && x < modal_x + modal_w && y >= modal_y && y < modal_y + modal_h;

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

    let in_modal = x >= modal_x && x < modal_x + modal_w && y >= modal_y && y < modal_y + modal_h;

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
    let in_modal = x >= modal_x && x < modal_x + modal_w && y >= modal_y && y < modal_y + modal_h;

    let total = state.docker_list_items.len();
    let header_height = crate::ui::layout::docker_list_header_height(modal_h);
    let visible = docker_list_visible_height(list_h, modal_h);
    let scroll = docker_list_scroll_offset(state.docker_list_selected, visible, total);

    let row_at = if y >= list_y.saturating_add(header_height)
        && y < list_y.saturating_add(list_h)
        && x >= list_x
        && x < list_x + list_w
    {
        let rel = y.saturating_sub(list_y + header_height) as usize;
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
                            vec![
                                ContextMenuAction::Inspect,
                                ContextMenuAction::DeleteContainer,
                            ]
                        }
                        DockerListKind::Volumes => {
                            vec![
                                ContextMenuAction::Inspect,
                                ContextMenuAction::DeleteVolume,
                                ContextMenuAction::ShowContainers,
                                ContextMenuAction::PruneVolumes,
                            ]
                        }
                    };
                    let (menu_x, menu_y) =
                        position_context_menu(x, y, items.len() + 1, width, height);
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
) -> (
    u16,
    u16,
    u16,
    u16,
    (u16, u16, u16, u16),
    (u16, u16, u16, u16),
) {
    let _ = state;
    let (area, yes, no) = crate::ui::layout::prune_confirmation(ratatui::layout::Rect::new(
        main_x, 0, main_width, height,
    ));
    (
        area.x,
        area.y,
        area.width,
        area.height,
        (yes.x, yes.y, yes.width, yes.height),
        (no.x, no.y, no.width, no.height),
    )
}

fn delete_confirm_layout(
    _state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (
    u16,
    u16,
    u16,
    u16,
    (u16, u16, u16, u16),
    (u16, u16, u16, u16),
) {
    let (area, yes, no) = crate::ui::layout::delete_confirmation(ratatui::layout::Rect::new(
        main_x, 0, main_width, height,
    ));
    (
        area.x,
        area.y,
        area.width,
        area.height,
        (yes.x, yes.y, yes.width, yes.height),
        (no.x, no.y, no.width, no.height),
    )
}

fn log_output_layout(
    state: &AppState,
    main_x: u16,
    main_width: u16,
    height: u16,
) -> (
    u16,
    u16,
    u16,
    u16,
    (u16, u16, u16, u16),
    (u16, u16, u16, u16),
) {
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
) -> (
    u16,
    u16,
    u16,
    u16,
    (u16, u16, u16, u16),
    (u16, u16, u16, u16),
) {
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

    let list_area = (
        x + 2,
        y + crate::ui::layout::docker_list_top_padding(height_box),
        width.saturating_sub(4),
        crate::ui::layout::docker_list_content_height(
            height_box,
            state.docker_list_kind == Some(DockerListKind::Volumes),
        ),
    );

    let button_w = 10u16;
    let button_h = 1u16;
    let button_y = y + height_box - 3;
    let button_x = x + (width.saturating_sub(button_w)) / 2;
    let close_area = (button_x, button_y, button_w, button_h);

    (x, y, width, height_box, list_area, close_area)
}

fn docker_list_visible_height(list_height: u16, modal_height: u16) -> usize {
    list_height.saturating_sub(crate::ui::layout::docker_list_header_height(modal_height)) as usize
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

#[cfg(test)]
#[path = "column_sort_tests.rs"]
mod column_sort_tests;

#[cfg(test)]
mod tests {
    use sysinfo::Pid;
    #[test]
    fn compact_volume_rows_below_the_header_keep_their_mouse_action_target() {
        let mut state = AppState::new();
        state.view_mode = ViewMode::Docker;
        state.term_width = 30;
        state.term_height = 10;
        state.docker_list_open = true;
        state.docker_list_kind = Some(DockerListKind::Volumes);
        state.docker_list_items = (0..2)
            .map(|id| crate::system::docker::DockerListItem {
                name: format!("db{id}"),
                id: format!("db{id}"),
                size: "9 GB".into(),
                ..Default::default()
            })
            .collect();
        state.docker_list_selected = 1;
        let (_, _, _, _, list, _) = docker_list_modal_layout(&state, 0, 30, 10);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: list.0 + 1,
            row: list.1 + 1,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(mouse, &mut state, &[], &[], &[], &[], 30, 10);
        assert!(
            matches!(&state.context_menu.unwrap().target,ContextMenuTarget::DockerVolume { name } if name=="db1")
        );
    }
    #[test]
    fn sort_menu_contains_keyboard_and_mouse_input_and_keeps_table_settings_independent() {
        let mut state = AppState::new();
        state.view_mode = ViewMode::Ports;
        state.term_width = 30;
        state.term_height = 10;
        let key = |code, state: &mut AppState| {
            handle_key_event(
                KeyEvent::new(code, KeyModifiers::NONE),
                state,
                &mut System::new(),
                &[],
                &[],
                &[],
                &[],
            )
        };
        key(KeyCode::Char('s'), &mut state);
        key(KeyCode::Down, &mut state);
        key(KeyCode::Enter, &mut state);
        assert_eq!(
            state.sort_for(crate::app::sorting::SortTarget::Ports).field,
            crate::app::sorting::SortField::Name
        );
        assert_eq!(state.sort_by, SortBy::Memory);
        state.view_mode = ViewMode::Node;
        state.node_tab = NodeTab::Pm2;
        state.pm2_available = true;
        key(KeyCode::Char('s'), &mut state);
        key(KeyCode::End, &mut state);
        let menu = state.sort_menu.unwrap();
        let bounds = ratatui::layout::Rect::new(0, 0, 30, 10);
        let (area, scroll) =
            crate::ui::layout::sort_menu_area(bounds, menu.target.fields().len(), menu.selected);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 2,
            row: area.y + 1 + (menu.selected - scroll) as u16,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(mouse, &mut state, &[], &[], &[], &[], 30, 10);
        assert!(state.sort_menu.is_none());
        assert_eq!(
            state.sort_for(crate::app::sorting::SortTarget::Pm2).field,
            crate::app::sorting::SortField::Uptime
        );
        assert_eq!(
            state.sort_for(crate::app::sorting::SortTarget::Node).field,
            crate::app::sorting::SortField::Pid
        );
    }
    #[test]
    fn volume_owner_shortcut_uses_exact_container_ids_and_unknown_owners_cannot_change_scope() {
        let mut state = AppState::new();
        state.docker_list_open = true;
        state.docker_list_kind = Some(DockerListKind::Volumes);
        state.docker_list_items = vec![crate::system::docker::DockerListItem {
            name: "data".into(),
            attachments: Some(vec![crate::system::docker::VolumeAttachment {
                id: "full-container-id".into(),
                container: "api".into(),
                ..Default::default()
            }]),
            ..Default::default()
        }];
        show_volume_containers(&mut state);
        assert_eq!(
            state.docker_volume_scope,
            Some(("data".into(), vec!["full-container-id".into()]))
        );
        assert!(!state.docker_list_open);
        assert_eq!(state.view_mode, ViewMode::Docker);
        handle_normal_mode(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut state,
            &mut System::new(),
            &[],
            &[],
        );
        assert!(state.docker_volume_scope.is_none());
        state.docker_list_open = true;
        state.docker_list_items[0].attachments = None;
        show_volume_containers(&mut state);
        assert!(state.docker_list_open);
        assert!(state.docker_volume_scope.is_none());
    }
    #[test]
    fn resource_menus_are_keyboard_operable_and_contain_focus_above_the_list() {
        let mut state = AppState::new();
        state.view_mode = ViewMode::Docker;
        state.term_width = 40;
        state.term_height = 16;
        state.docker_list_open = true;
        state.docker_list_kind = Some(DockerListKind::Volumes);
        state.docker_list_items = vec![crate::system::docker::DockerListItem {
            name: "café-db".into(),
            id: "café-db".into(),
            size: "950 GB".into(),
            activity: None,
            detail_left: String::new(),
            detail_right: String::new(),
            ..Default::default()
        }];
        for key in [KeyCode::F(10), KeyCode::Down, KeyCode::Enter] {
            handle_key_event(
                KeyEvent::new(key, KeyModifiers::NONE),
                &mut state,
                &mut System::new(),
                &[],
                &[],
                &[],
                &[],
            );
        }
        assert_eq!(state.docker_list_selected, 0);
        assert!(state.context_menu.is_none());
        assert!(state.docker_list_open);
        assert_eq!(state.pending_delete.as_ref().unwrap().name, "café-db");
        handle_key_event(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut state,
            &mut System::new(),
            &[],
            &[],
            &[],
            &[],
        );
        assert!(state.pending_delete.is_none());
    }

    #[test]
    fn grouped_port_targets_use_the_rendered_item_index_instead_of_item_ordinal() {
        let mut state = AppState::new();
        let port = |name: &str, pid| crate::system::ports::PortInfo {
            proto: "tcp".into(),
            port: 8080,
            internal_port: None,
            pid: Pid::from_u32(pid),
            name: name.into(),
            exe_path: "-".into(),
            container_id: None,
            group_name: None,
            project_name: None,
        };
        let ports = vec![port("first", 10), port("second", 20)];
        state.visible_port_indices = vec![None, Some(1), None, Some(0)];
        assert_eq!(find_port_index_for_row(&state, 1, &ports), Some(1));
        assert_eq!(find_port_index_for_row(&state, 3, &ports), Some(0));
    }

    #[test]
    fn compact_ports_and_node_clicks_and_hover_exclude_header_and_bottom_border() {
        for height in [10, 16, 24] {
            let mut state = AppState::new();
            state.view_mode = ViewMode::Ports;
            state.visible_ports = (10..30).map(Pid::from_u32).collect();
            state.visible_port_indices = (0..20).map(Some).collect();
            let table = crate::ui::layout::resource_table(40, height);
            handle_main_click(&mut state, 3, table.y + 3, 40, height, &[]);
            assert_eq!(state.selected, 1);
            handle_main_hover(&mut state, 3, table.bottom() - 1, height, &[]);
            assert!(state.hover_row.is_none());
            handle_ports_right_click(&mut state, 3, table.bottom() - 1, 40, height, 0, &[]);
            assert!(state.context_menu.is_none());
            state.view_mode = ViewMode::Node;
            let table =
                crate::ui::layout::node_layout(ratatui::layout::Rect::new(0, 0, 40, height))[4];
            state.visible_pids = (10..30).map(Pid::from_u32).collect();
            state.visible_node_selectable = vec![true; 20];
            handle_node_right_click(&mut state, 3, table.bottom() - 1, 40, height, 0, &[], &[]);
            assert!(state.context_menu.is_none());
            handle_node_right_click(&mut state, 3, table.y + 3, 40, height, 0, &[], &[]);
            assert_eq!(state.selected, 1);
            assert!(matches!(
                state.context_menu.as_ref().unwrap().target,
                ContextMenuTarget::Process { pid: 11, .. }
            ));
            state.context_menu = None;
            state.pm2_available = true;
            state.node_tab = NodeTab::Pm2;
            let (pm2, _) = crate::ui::layout::node_tables(
                ratatui::layout::Rect::new(0, 0, 40, height),
                NodeTab::Pm2,
            );
            handle_main_click(&mut state, 3, pm2.y + 2, 40, height, &[0, 1]);
            assert_eq!(state.pm2_selected, 0);
            assert_eq!(state.node_tab, NodeTab::Pm2);
        }
    }

    use super::*;

    #[test]
    fn node_tabs_default_to_processes_and_keep_selections_sorts_and_scroll_when_switching() {
        let mut state = AppState::new();
        let mut system = System::new();
        state.term_width = 100;
        state.term_height = 24;
        state.focus = Focus::Sidebar;
        state.set_view(ViewMode::Node);
        assert_eq!(state.node_tab, NodeTab::Processes);
        assert_eq!(state.focus, Focus::Sidebar);
        state.focus = Focus::Main;
        state.selected = 4;
        state.node_scroll = 3;
        state.pm2_selected = 2;
        state.pm2_scroll = 1;
        state.hover_row = Some(4);
        state.apply_sort(
            crate::app::sorting::SortTarget::Node,
            crate::app::sorting::TableSort::new(
                crate::app::sorting::SortField::Memory,
                crate::app::SortOrder::Desc,
            ),
        );
        let sorts = state.table_sorts;
        // Switching works before PM2 is available and without rows in either table.
        handle_normal_mode(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut state,
            &mut system,
            &[],
            &[],
        );
        assert_eq!(state.node_tab, NodeTab::Pm2);
        assert!(state.hover_row.is_none());
        state.pm2_hover_row = Some(2);
        handle_normal_mode(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &mut state,
            &mut system,
            &[],
            &[],
        );
        assert_eq!(state.node_tab, NodeTab::Processes);
        assert!(state.pm2_hover_row.is_none());
        assert_eq!(
            (
                state.selected,
                state.node_scroll,
                state.pm2_selected,
                state.pm2_scroll
            ),
            (4, 3, 2, 1)
        );
        assert_eq!(state.table_sorts, sorts);
        state.set_node_tab(NodeTab::Pm2);
        state.set_view(ViewMode::Node);
        assert_eq!(state.node_tab, NodeTab::Pm2);
        state.set_view(ViewMode::Ports);
        state.set_view(ViewMode::Node);
        assert_eq!(state.node_tab, NodeTab::Processes);
    }

    #[test]
    fn node_tab_clicks_and_empty_pm2_actions_never_target_the_hidden_native_table() {
        for (width, height) in [(100, 24), (40, 16), (30, 10)] {
            let mut state = AppState::new();
            let mut system = System::new();
            state.term_width = width;
            state.term_height = height;
            state.set_view(ViewMode::Node);
            state.visible_pids = vec![Pid::from_u32(u32::MAX); 4];
            state.visible_node_selectable = vec![true; 4];
            state.selected = 2;
            let main =
                crate::ui::layout::main_area(ratatui::layout::Rect::new(0, 0, width, height));
            let chunks = crate::ui::layout::node_layout(main);
            let tabs = crate::ui::layout::node_tab_areas(chunks[1]);
            let click = |area: ratatui::layout::Rect, state: &mut AppState| {
                handle_mouse_event(
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: area.x + 1,
                        row: area.y,
                        modifiers: KeyModifiers::NONE,
                    },
                    state,
                    &[],
                    &[],
                    &[],
                    &[],
                    width,
                    height,
                );
            };
            click(tabs[1], &mut state);
            assert_eq!(state.node_tab, NodeTab::Pm2);
            let table = chunks[4];
            let body_y = table.y + 2;
            handle_main_click(&mut state, 3, body_y, main.width, height, &[]);
            handle_main_hover(&mut state, 3, body_y, height, &[]);
            assert!(state.hover_row.is_none());
            handle_node_right_click(
                &mut state,
                main.x + 3,
                body_y,
                width,
                height,
                main.x,
                &[],
                &[],
            );
            open_keyboard_menu(&mut state, &[], &[], &[], &[]);
            assert!(state.context_menu.is_none());
            for code in [
                KeyCode::Down,
                KeyCode::End,
                KeyCode::Char('e'),
                KeyCode::Char('k'),
            ] {
                handle_normal_mode(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    &mut state,
                    &mut system,
                    &[],
                    &[],
                );
            }
            assert_eq!(state.selected, 2);
            assert!(state.env_request.is_none());
            assert!(!state.env_modal_open);
            assert!(state.pending_operations.is_empty());
            click(tabs[0], &mut state);
            assert_eq!(state.node_tab, NodeTab::Processes);
            handle_node_right_click(
                &mut state,
                main.x + 3,
                body_y,
                width,
                height,
                main.x,
                &[],
                &[],
            );
            assert!(matches!(
                state.context_menu.as_ref().unwrap().target,
                ContextMenuTarget::Process { pid: u32::MAX, .. }
            ));
        }
    }

    #[test]
    fn process_memory_table_mouse_rows_match_rendering_at_short_heights() {
        for height in [16, 20, 24, 36] {
            let mut state = AppState::new();
            state.view_mode = ViewMode::Process;
            state.term_width = 100;
            state.visible_pids = (10..30).map(sysinfo::Pid::from_u32).collect();
            let table = crate::ui::layout::process_table(80, height);
            handle_main_click(&mut state, 5, table.y + 3, 80, height, &[]);
            assert_eq!(state.selected, 1);
            handle_main_click(&mut state, 5, table.bottom() - 1, 80, height, &[]);
            assert_eq!(state.selected, 1);
            handle_main_hover(&mut state, 5, table.bottom() - 1, height, &[]);
            assert!(state.hover_row.is_none());
            handle_process_right_click(&mut state, 5, table.y + 2, 80, height, 0);
            assert!(matches!(
                state.context_menu.as_ref().map(|menu| &menu.target),
                Some(ContextMenuTarget::Process { pid: 10, .. })
            ));
        }
    }

    #[test]
    fn another_delete_cannot_start_while_a_delete_is_pending() {
        let mut state = AppState::new();
        state.delete_in_progress = Some(DeleteProgress {
            label: "volume large-volume".into(),
            started_at: std::time::Instant::now(),
        });
        request_delete_confirmation(
            &mut state,
            DeleteKind::Volume,
            "other".into(),
            "other".into(),
        );
        assert!(state.pending_delete.is_none());
        start_delete_action(
            &mut state,
            DeleteConfirm {
                kind: DeleteKind::Volume,
                name: "other".into(),
                id: "other".into(),
            },
        );
        assert_eq!(
            state.delete_in_progress.as_ref().unwrap().label,
            "volume large-volume"
        );
    }

    #[test]
    fn long_volume_menu_clicks_and_delete_confirmation_stay_within_terminal() {
        use ratatui::layout::Rect;
        for width in [30, 40, 60, 61, 100, 140] {
            let bounds = crate::ui::layout::main_area(Rect::new(0, 0, width, 16));
            let menu = ContextMenu {
                x: width - 5,
                y: 15,
                items: vec![ContextMenuAction::Inspect, ContextMenuAction::DeleteVolume],
                hover: None,
                target: ContextMenuTarget::DockerVolume {
                    name: "a".repeat(64),
                },
                is_group: false,
                header: Some(format!("Volume: {}", "a".repeat(64))),
            };
            let labels: Vec<_> = menu
                .items
                .iter()
                .map(|action| action.label(false))
                .collect();
            let area = crate::ui::layout::context_menu_area(
                bounds,
                menu.x,
                menu.y,
                &labels,
                menu.header.as_deref(),
            );
            assert!(area.right() <= bounds.right());
            assert!(area.bottom() <= bounds.bottom());
            for x in [area.x + 1, area.right() - 2] {
                assert_eq!(
                    get_menu_action_at(&menu, x, area.y + 3, bounds),
                    Some(ContextMenuAction::DeleteVolume)
                );
            }
            assert_eq!(get_menu_action_at(&menu, area.x, area.y + 3, bounds), None);
            let (area, yes, no) = crate::ui::layout::delete_confirmation(bounds);
            assert!(area.right() <= bounds.right());
            assert!(yes.x >= area.x && no.right() <= area.right());
        }
    }

    #[test]
    fn docker_sizes_include_spaced_units_but_exclude_virtual_image_size() {
        assert_eq!(docker_size_bytes("1.25 GB"), 1_250_000_000);
        assert_eq!(docker_size_bytes("36 B"), 36);
        assert_eq!(docker_size_bytes("35.58 kB (virtual 209.5 MB)"), 35_580);
        assert_eq!(docker_size_bytes("Unknown"), 0);
    }

    #[test]
    fn docker_mouse_selection_tracks_compact_layout_and_ignores_footer() {
        for height in [16, 20, 24, 36] {
            let mut state = AppState::new();
            state.view_mode = ViewMode::Docker;
            state.term_width = 100;
            state.docker_rows = (0..20)
                .map(|index| DockerRow::Item {
                    index,
                    prefix: String::new(),
                })
                .collect();
            let table = crate::ui::layout::docker_table(80, height);
            handle_main_click(&mut state, 5, table.y + 3, 80, height, &[]);
            assert_eq!(state.docker_selected_row, 1);
            handle_main_click(&mut state, 5, height - 1, 80, height, &[]);
            assert_eq!(state.docker_selected_row, 1);
            handle_main_hover(&mut state, 5, height - 1, height, &[]);
            assert!(state.hover_row.is_none());
        }
    }

    #[test]
    fn compact_disk_actions_use_visible_rows_only() {
        assert_eq!(crate::ui::layout::docker_disk_row(80, 20, 9), None);
        assert_eq!(crate::ui::layout::docker_disk_row(80, 24, 6), Some(0));
        assert_eq!(crate::ui::layout::docker_disk_row(80, 36, 6), Some(0));
    }

    #[test]
    fn escape_cancels_a_loading_inspect_above_a_resource_list() {
        let mut state = AppState::new();
        state.docker_list_open = true;
        state.log_in_progress = Some("inspect".into());
        handle_key_event(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut state,
            &mut System::new(),
            &[],
            &[],
            &[],
            &[],
        );
        assert!(state.log_in_progress.is_none());
        assert!(state.docker_list_open);
    }
}
