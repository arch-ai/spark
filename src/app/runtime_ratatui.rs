//! Ratatui-based runtime for the Spark system manager

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sysinfo::{Disks, Pid, System};

use crate::app::input::{handle_key_event, handle_mouse_event};
use crate::app::{AppState, ViewMode};
use crate::system::{docker, node, ports, process};
use crate::ui::render_ratatui;

pub fn run_ratatui(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut system = System::new_all();
    system.refresh_all();

    let docker_worker = docker::start_docker_stats_worker(Duration::from_secs(2));
    let docker_df_worker = docker::start_docker_df_worker(Duration::from_secs(10));
    let ports_worker = ports::start_ports_worker(Duration::from_secs(5));

    let mut state = AppState::new();
    update_system_snapshot(&mut state, &system);
    maybe_refresh_user_cache(&mut state);

    let tick_rate = Duration::from_millis(1000);
    let input_poll = Duration::from_millis(60);
    let mut last_tick = Instant::now();
    let mut needs_render = true;

    // Process view cache
    let mut process_cache: HashMap<Pid, process::ProcInfo> = HashMap::new();
    let mut rows_cache: Vec<process::TreeRow> = Vec::new();
    let mut process_dirty = true;

    // Docker view cache - docker_raw uses Arc for efficient snapshot without cloning
    let mut docker_raw: Arc<Vec<docker::ContainerInfo>> = Arc::new(Vec::new());
    let mut docker_view: Vec<docker::ContainerInfo> = Vec::new();
    let mut docker_rows: Vec<docker::DockerRow> = Vec::new();
    let mut docker_dirty = true;
    let mut last_docker_pull = Instant::now() - Duration::from_secs(60);
    let mut last_docker_df_pull = Instant::now();

    // Ports view cache - uses background worker, so just need mutable state for filtering/grouping
    let mut ports_raw: Arc<Vec<ports::PortInfo>> = Arc::new(Vec::new());
    let mut ports_cache: Vec<ports::PortInfo> = Vec::new();
    let mut ports_rows: Vec<ports::PortRow> = Vec::new();

    // Node view cache
    let mut node_view: Vec<node::NodeProcessInfo> = Vec::new();
    let mut node_rows: Vec<node::NodeRow> = Vec::new();
    let mut node_dirty = true;

    const PM2_CHECK_INTERVAL: Duration = Duration::from_secs(30);

    // Cache terminal size to avoid syscalls on every mouse event
    let size = terminal.size().unwrap_or_else(|_| terminal.size().unwrap_or_default());
    let mut term_width = size.width;
    let mut term_height = size.height;

    loop {
        // Handle input events
        if event::poll(input_poll)? {
            let ev = event::read()?;

            if let Event::Key(key) = ev {
                let prev_filter = state.active_filter().to_string();
                let prev_sort_by = state.sort_by;
                let prev_sort_order = state.sort_order;
                let prev_zoom = state.zoom;
                let prev_view = state.view_mode;

                if handle_key_event(key, &mut state, &mut system) {
                    break;
                }

                let filter_changed = state.active_filter() != prev_filter;
                let sort_changed = state.sort_by != prev_sort_by || state.sort_order != prev_sort_order;
                let zoom_changed = state.zoom != prev_zoom;
                let view_changed = state.view_mode != prev_view;

                if filter_changed {
                    match state.view_mode {
                        ViewMode::Process => process_dirty = true,
                        ViewMode::Docker => docker_dirty = true,
                        ViewMode::DockerEnv => {}
                        ViewMode::Ports => {
                            // Filter is applied on worker snapshot, need to re-process
                        }
                        ViewMode::Node => node_dirty = true,
                    }
                }
                if sort_changed {
                    process_dirty = true;
                    docker_dirty = true;
                }
                if zoom_changed {
                    process_dirty = true;
                }
                if view_changed {
                    process_dirty = true;
                    docker_dirty = true;
                    node_dirty = true;
                }

                needs_render = true;
            }

            if let Event::Mouse(mouse) = ev {
                let prev_view = state.view_mode;
                let mouse_needs_render = handle_mouse_event(mouse, &mut state, &docker_view, &ports_cache, term_width, term_height);
                let view_changed = state.view_mode != prev_view;

                if view_changed {
                    process_dirty = true;
                    docker_dirty = true;
                    node_dirty = true;
                }

                if mouse_needs_render || view_changed {
                    needs_render = true;
                }
            }
        }

        // Periodic system refresh
        if last_tick.elapsed() >= tick_rate {
            refresh_system(&mut system);
            update_system_snapshot(&mut state, &system);
            last_tick = Instant::now();

            match state.view_mode {
                ViewMode::Process => process_dirty = true,
                ViewMode::Docker | ViewMode::DockerEnv => {}
                ViewMode::Ports => {}
                ViewMode::Node => node_dirty = true,
            }
            needs_render = true;
        }

        if state.clear_expired_message() {
            needs_render = true;
        }

        if state.check_completed_operations() {
            needs_render = true;
        }

        if state.tick_spinner() {
            needs_render = true;
        }

        // Update data based on current view
        match state.view_mode {
            ViewMode::Process => {
                if process_dirty {
                    maybe_refresh_user_cache(&mut state);
                    process_cache = process::collect_processes(
                        &system,
                        &state.process_filter,
                        &state.user_cache,
                        state.zoom,
                    );
                    rows_cache = process::build_tree_rows(
                        &process_cache,
                        state.sort_by,
                        state.sort_order,
                        state.zoom,
                    );
                    clamp_selection(&mut state, rows_cache.len());
                    state.visible_pids = rows_cache.iter().map(|row| row.pid).collect();
                    process_dirty = false;
                }
            }
            ViewMode::Docker => {
                // Snapshot docker system df from background worker (non-blocking)
                if last_docker_df_pull.elapsed() >= Duration::from_millis(500) {
                    state.docker_system_df = docker_df_worker.snapshot();
                    last_docker_df_pull = Instant::now();
                    needs_render = true;
                }

                if last_docker_pull.elapsed() >= Duration::from_millis(500) {
                    docker_raw = docker_worker.snapshot();
                    docker_dirty = true;
                    last_docker_pull = Instant::now();

                    if state.update_pending_with_containers(&docker_raw) {
                        needs_render = true;
                    }
                }

                if docker_dirty {
                    // Clone the inner Vec only when we need to apply filters
                    docker_view = (*docker_raw).clone();
                    docker::apply_container_filter(&mut docker_view, &state.docker_filter);
                    let (grouped, rows) = docker::group_containers(docker_view);
                    docker_view = grouped;
                    docker_rows = rows.clone();
                    state.docker_rows = rows;
                    state.docker_total = docker_raw.len();
                    state.docker_filtered_out = state.docker_total.saturating_sub(docker_view.len());
                    clamp_docker_selection(&mut state);

                    // Use extend for more efficient vector population
                    state.visible_containers.clear();
                    state.visible_containers.extend(docker_view.iter().map(|c| c.id.clone()));
                    state.visible_container_names.clear();
                    state.visible_container_names.extend(docker_view.iter().map(|c| c.name.clone()));
                    state.visible_container_ports_public.clear();
                    state.visible_container_ports_public.extend(docker_view.iter().map(|c| c.port_public.clone()));
                    state.visible_container_ports_internal.clear();
                    state.visible_container_ports_internal.extend(docker_view.iter().map(|c| c.port_internal.clone()));
                    state.visible_container_group_name.clear();
                    state.visible_container_group_name.extend(docker_view.iter().map(|c| c.group_name.clone()));
                    state.visible_container_group_path.clear();
                    state.visible_container_group_path.extend(docker_view.iter().map(|c|
                        c.group_path.clone().unwrap_or_else(|| "-".to_string())
                    ));
                    docker_dirty = false;
                }
            }
            ViewMode::DockerEnv => {}
            ViewMode::Ports => {
                // Snapshot ports from background worker (non-blocking)
                let new_ports = ports_worker.snapshot();

                // Only update if data changed (pointer comparison)
                if !Arc::ptr_eq(&new_ports, &ports_raw) {
                    ports_raw = new_ports;
                    ports_cache = (*ports_raw).clone();
                    crate::util::apply_filter(&mut ports_cache, &state.ports_filter);
                    ports_rows = ports::group_ports(&ports_cache);
                    clamp_selection(&mut state, ports_rows.len());

                    state.visible_ports.clear();
                    state.visible_ports_container_ids.clear();
                    state.visible_ports.reserve(ports_rows.len());
                    state.visible_ports_container_ids.reserve(ports_rows.len());

                    for row in &ports_rows {
                        match row {
                            ports::PortRow::Group { .. } => {
                                state.visible_ports.push(Pid::from_u32(0));
                                state.visible_ports_container_ids.push(None);
                            }
                            ports::PortRow::Item { index, .. } => {
                                let port = &ports_cache[*index];
                                state.visible_ports.push(port.pid);
                                state.visible_ports_container_ids.push(port.container_id.clone());
                            }
                        }
                    }
                    clamp_ports_selection(&mut state);
                    needs_render = true;
                }
            }
            ViewMode::Node => {
                if node_dirty {
                    if state.pm2_last_check.elapsed() >= PM2_CHECK_INTERVAL {
                        state.pm2_available = node::is_pm2_running();
                        state.pm2_last_check = Instant::now();
                    }

                    let node_cache = node::collect_node_processes(&system, &state.node_filter);
                    let mut node_main = Vec::new();
                    let mut node_utils = Vec::new();
                    for proc in node_cache {
                        if node::is_node_util(&proc) {
                            node_utils.push(proc);
                        } else {
                            node_main.push(proc);
                        }
                    }

                    let utils_offset = node_main.len();
                    node_view = node_main;
                    node_view.extend(node_utils);

                    if node_view.is_empty() {
                        node_rows.clear();
                    } else if utils_offset == 0 {
                        node_rows.clear();
                        node_rows.push(node::NodeRow::UtilsSpacer);
                        node_rows.push(node::NodeRow::UtilsTitle);
                        node_rows.push(node::NodeRow::UtilsTop);
                        node_rows.push(node::NodeRow::UtilsHeader);
                        node_rows.push(node::NodeRow::UtilsSeparator);
                        node_rows.extend(node::group_node_processes(&node_view, 0));
                    } else {
                        node_rows = node::group_node_processes(&node_view[..utils_offset], 0);
                        if utils_offset < node_view.len() {
                            node_rows.push(node::NodeRow::UtilsSpacer);
                            node_rows.push(node::NodeRow::UtilsTitle);
                            node_rows.push(node::NodeRow::UtilsTop);
                            node_rows.push(node::NodeRow::UtilsHeader);
                            node_rows.push(node::NodeRow::UtilsSeparator);
                            node_rows.extend(node::group_node_processes(&node_view[utils_offset..], utils_offset));
                        }
                    }

                    clamp_selection(&mut state, node_rows.len());
                    state.visible_pids.clear();
                    state.visible_node_selectable.clear();
                    state.visible_pids.reserve(node_rows.len());
                    state.visible_node_selectable.reserve(node_rows.len());

                    for row in &node_rows {
                        match row {
                            node::NodeRow::Item { index } => {
                                let proc = &node_view[*index];
                                state.visible_pids.push(proc.pid);
                                state.visible_node_selectable.push(true);
                            }
                            _ => {
                                state.visible_pids.push(Pid::from_u32(0));
                                state.visible_node_selectable.push(false);
                            }
                        }
                    }
                    clamp_node_selection(&mut state);
                    node_dirty = false;
                }
            }
        }

        // Render using ratatui
        if needs_render {
            // Update cached terminal size
            let size = terminal.size()?;
            term_width = size.width;
            term_height = size.height;
            let visible_height = adjust_visible_height(&state, size.height);
            let total = match state.view_mode {
                ViewMode::Process => rows_cache.len(),
                ViewMode::Docker => docker_rows.len(),
                ViewMode::Ports => ports_rows.len(),
                ViewMode::Node => node_rows.len(),
                ViewMode::DockerEnv => 0,
            };
            state.adjust_scroll(visible_height, total);

            terminal.draw(|frame| {
                render_ratatui(
                    frame,
                    &state,
                    &process_cache,
                    &rows_cache,
                    &docker_view,
                    &docker_rows,
                    &ports_cache,
                    &ports_rows,
                    &node_view,
                    &node_rows,
                );
            })?;

            // Update hover render timestamp for throttling
            state.last_hover_render = Instant::now();
            needs_render = false;
        }
    }

    Ok(())
}

/// Calculate visible height for table rows based on view mode and terminal height
fn adjust_visible_height(state: &AppState, height: u16) -> usize {
    // Layout offsets:
    // Process: title(3) + header(1) + search(3) + bars(4) + table_border_header(2) + help(2) = 15
    // Docker: title(3) + header(1) + search(3) + df_stats(7) + table_border_header(2) + help(2) = 18
    // Ports/Node: title(3) + header(1) + search(3) + table_border_header(2) + help(2) = 11
    let overhead = match state.view_mode {
        ViewMode::Process => 15u16,
        ViewMode::Docker => 18,
        ViewMode::Ports | ViewMode::Node => 11,
        ViewMode::DockerEnv => return 0,
    };
    (height.saturating_sub(overhead)) as usize
}

fn refresh_system(system: &mut System) {
    system.refresh_processes();
    system.refresh_cpu();
    system.refresh_memory();
}

fn update_system_snapshot(state: &mut AppState, system: &System) {
    state.cpu_usage = system.global_cpu_info().cpu_usage();
    state.mem_total = system.total_memory();
    state.mem_available = system.available_memory();
    state.swap_total = system.total_swap();
    state.swap_used = system.used_swap();

    let disks = Disks::new_with_refreshed_list();
    let root = disks
        .iter()
        .find(|disk| disk.mount_point() == std::path::Path::new("/"))
        .or_else(|| disks.iter().next());
    if let Some(disk) = root {
        state.disk_total = disk.total_space();
        state.disk_available = disk.available_space();
    } else {
        state.disk_total = 0;
        state.disk_available = 0;
    }
}

fn maybe_refresh_user_cache(state: &mut AppState) {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
    if state.user_last_refresh.elapsed() >= REFRESH_INTERVAL {
        let users = sysinfo::Users::new_with_refreshed_list();
        state.user_cache.clear();
        for user in users.iter() {
            state.user_cache.insert(user.id().clone(), user.name().to_string());
        }
        state.user_last_refresh = Instant::now();
    }
}

fn clamp_selection(state: &mut AppState, len: usize) {
    if len == 0 {
        state.selected = 0;
    } else if state.selected >= len {
        state.selected = len - 1;
    }
}

fn clamp_docker_selection(state: &mut AppState) {
    let len = state.docker_rows.len();
    if len == 0 {
        state.docker_selected_row = 0;
    } else if state.docker_selected_row >= len {
        state.docker_selected_row = len - 1;
    }
}

fn clamp_ports_selection(state: &mut AppState) {
    let len = state.visible_ports.len();
    if len == 0 {
        state.selected = 0;
        return;
    }
    if state.selected >= len {
        state.selected = len - 1;
    }
    while state.selected > 0 && state.is_ports_group_row(state.selected) {
        state.selected -= 1;
    }
    if state.is_ports_group_row(state.selected) {
        for i in 0..len {
            if !state.is_ports_group_row(i) {
                state.selected = i;
                break;
            }
        }
    }
}

fn clamp_node_selection(state: &mut AppState) {
    let len = state.visible_pids.len();
    if len == 0 {
        state.selected = 0;
        return;
    }
    if state.selected >= len {
        state.selected = len - 1;
    }
    while state.selected > 0 && !state.is_node_selectable_row(state.selected) {
        state.selected -= 1;
    }
    if !state.is_node_selectable_row(state.selected) {
        for i in 0..len {
            if state.is_node_selectable_row(i) {
                state.selected = i;
                break;
            }
        }
    }
}
