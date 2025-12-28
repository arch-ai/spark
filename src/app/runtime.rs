use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use sysinfo::{Pid, System, Users};

use crate::app::input::handle_key_event;
use crate::app::{AppState, ViewMode};
use crate::system::{docker, node, ports, process};
use crate::ui;

pub fn run(stdout: &mut io::Stdout) -> io::Result<()> {
    let mut system = System::new_all();
    system.refresh_all();

    let docker_worker = docker::start_docker_stats_worker(Duration::from_secs(2));

    let mut state = AppState::new();
    update_system_snapshot(&mut state, &system);
    maybe_refresh_user_cache(&mut state);
    let tick_rate = Duration::from_millis(1000);
    let input_poll = Duration::from_millis(60);
    let mut last_tick = Instant::now();
    let mut needs_render = true;

    let mut process_cache: HashMap<Pid, process::ProcInfo> = HashMap::new();
    let mut rows_cache: Vec<process::TreeRow> = Vec::new();
    let mut process_dirty = true;

    let mut docker_raw: Vec<docker::ContainerInfo> = Vec::new();
    let mut docker_view: Vec<docker::ContainerInfo> = Vec::new();
    let mut docker_rows: Vec<docker::DockerRow> = Vec::new();
    let mut docker_dirty = true;
    let mut last_docker_pull = Instant::now() - Duration::from_secs(60);
    let mut ports_cache: Vec<ports::PortInfo> = Vec::new();
    let mut ports_rows: Vec<ports::PortRow> = Vec::new();
    let mut ports_dirty = true;

    let mut node_view: Vec<node::NodeProcessInfo> = Vec::new();
    let mut node_rows: Vec<node::NodeRow> = Vec::new();
    let mut node_dirty = true;
    let mut pm2_available = false;

    loop {
        if event::poll(input_poll)? {
            if let Event::Key(key) = event::read()? {
                let prev_filter = state.active_filter().to_string();
                let prev_sort_by = state.sort_by;
                let prev_sort_order = state.sort_order;
                let prev_zoom = state.zoom;
                let prev_view = state.view_mode;

                if handle_key_event(key, &mut state, &mut system) {
                    break;
                }

                let filter_changed = state.active_filter() != prev_filter;
                let sort_changed =
                    state.sort_by != prev_sort_by || state.sort_order != prev_sort_order;
                let zoom_changed = state.zoom != prev_zoom;
                let view_changed = state.view_mode != prev_view;

                if filter_changed {
                    match state.view_mode {
                        ViewMode::Process => process_dirty = true,
                        ViewMode::Docker => docker_dirty = true,
                        ViewMode::DockerEnv => {}
                        ViewMode::Ports => ports_dirty = true,
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
                    ports_dirty = true;
                    node_dirty = true;
                }

                needs_render = true;
            }
        }

        if last_tick.elapsed() >= tick_rate {
            refresh_system(&mut system);
            update_system_snapshot(&mut state, &system);
            last_tick = Instant::now();
            // Only mark dirty for the active view to avoid unnecessary data collection
            match state.view_mode {
                ViewMode::Process => process_dirty = true,
                ViewMode::Docker | ViewMode::DockerEnv => {
                    // Docker data is refreshed via worker, no need to mark dirty here
                }
                ViewMode::Ports => ports_dirty = true,
                ViewMode::Node => node_dirty = true,
            }
            needs_render = true;
        }

        if state.clear_expired_message() {
            needs_render = true;
        }

        match state.view_mode {
            ViewMode::Process => {
                if process_dirty {
                    maybe_refresh_container_cache(&mut state);
                    maybe_refresh_user_cache(&mut state);
                    process_cache = process::collect_processes(
                        &system,
                        &state.process_filter,
                        &state.container_cache,
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
                    state.visible_containers.clear();
                    state.visible_container_names.clear();
                    state.visible_container_ports_public.clear();
                    state.visible_container_ports_internal.clear();
                    state.visible_container_group_name.clear();
                    state.visible_container_group_path.clear();
                    state.visible_ports.clear();
                    state.visible_ports_container_ids.clear();
                    process_dirty = false;
                    needs_render = true;
                }

                if needs_render {
                    ui::render_processes(stdout, &state, &process_cache, &rows_cache)?;
                    needs_render = false;
                }
            }
            ViewMode::Docker => {
                if last_docker_pull.elapsed() >= Duration::from_millis(500) {
                    docker_raw = docker_worker.snapshot();
                    docker_dirty = true;
                    last_docker_pull = Instant::now();
                }

                if docker_dirty {
                    docker_view = docker_raw.clone();
                    docker::apply_container_filter(&mut docker_view, &state.docker_filter);
                    let (grouped, rows) =
                        docker::group_containers(docker_view, state.sort_by, state.sort_order);
                    docker_view = grouped;
                    docker_rows = rows;
                    state.docker_total = docker_raw.len();
                    state.docker_filtered_out =
                        state.docker_total.saturating_sub(docker_view.len());
                    clamp_selection(&mut state, docker_view.len());

                    // Pre-allocate and fill all vectors in a single iteration
                    let len = docker_view.len();
                    state.visible_containers.clear();
                    state.visible_containers.reserve(len);
                    state.visible_container_names.clear();
                    state.visible_container_names.reserve(len);
                    state.visible_container_ports_public.clear();
                    state.visible_container_ports_public.reserve(len);
                    state.visible_container_ports_internal.clear();
                    state.visible_container_ports_internal.reserve(len);
                    state.visible_container_group_name.clear();
                    state.visible_container_group_name.reserve(len);
                    state.visible_container_group_path.clear();
                    state.visible_container_group_path.reserve(len);

                    for container in &docker_view {
                        state.visible_containers.push(container.id.clone());
                        state.visible_container_names.push(container.name.clone());
                        state.visible_container_ports_public.push(container.port_public.to_string());
                        state.visible_container_ports_internal.push(container.port_internal.to_string());
                        state.visible_container_group_name.push(container.group_name.to_string());
                        state.visible_container_group_path.push(
                            container.group_path.clone().unwrap_or_else(|| "-".to_string())
                        );
                    }

                    state.visible_pids.clear();
                    state.visible_ports.clear();
                    state.visible_ports_container_ids.clear();
                    docker_dirty = false;
                    needs_render = true;
                }

                if needs_render {
                    ui::render_containers(stdout, &state, &docker_view, &docker_rows)?;
                    needs_render = false;
                }
            }
            ViewMode::DockerEnv => {
                if needs_render {
                    ui::render_envs(stdout, &state)?;
                    needs_render = false;
                }
            }
            ViewMode::Ports => {
                if ports_dirty {
                    ports_cache = ports::collect_ports(&system);
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
                            ports::PortRow::Item { index } => {
                                let port = &ports_cache[*index];
                                state.visible_ports.push(port.pid);
                                state.visible_ports_container_ids.push(port.container_id.clone());
                            }
                        }
                    }
                    clamp_ports_selection(&mut state);
                    state.visible_pids.clear();
                    state.visible_containers.clear();
                    ports_dirty = false;
                    needs_render = true;
                }

                if needs_render {
                    ui::render_ports(stdout, &state, &ports_cache, &ports_rows)?;
                    needs_render = false;
                }
            }
            ViewMode::Node => {
                if node_dirty {
                    pm2_available = node::is_pm2_running();

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
                            node_rows.extend(node::group_node_processes(
                                &node_view[utils_offset..],
                                utils_offset,
                            ));
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
                    state.visible_containers.clear();
                    state.visible_ports.clear();

                    node_dirty = false;
                    needs_render = true;
                }

                if needs_render {
                    ui::render_node_processes(stdout, &state, &node_view, &node_rows, pm2_available)?;
                    needs_render = false;
                }
            }
        }
    }

    Ok(())
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
}

fn maybe_refresh_container_cache(state: &mut AppState) {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
    if state.container_last_refresh.elapsed() < REFRESH_INTERVAL {
        return;
    }

    state.container_last_refresh = Instant::now();
    if let Some(cache) = docker::load_docker_container_cache() {
        state.container_cache = cache;
    }
}

fn maybe_refresh_user_cache(state: &mut AppState) {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
    if state.user_last_refresh.elapsed() < REFRESH_INTERVAL {
        return;
    }

    state.user_last_refresh = Instant::now();
    let users = Users::new_with_refreshed_list();
    let mut cache = HashMap::new();
    for user in users.list() {
        cache.insert(user.id().clone(), user.name().to_string());
    }
    state.user_cache = cache;
}

fn clamp_selection(state: &mut AppState, list_len: usize) {
    if list_len == 0 {
        state.selected = 0;
    } else if state.selected >= list_len {
        state.selected = list_len - 1;
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
    if !state.is_ports_group_row(state.selected) {
        return;
    }
    if let Some(next) = find_next_ports_row(state, state.selected, 1) {
        state.selected = next;
    } else if let Some(prev) = find_next_ports_row(state, state.selected, -1) {
        state.selected = prev;
    }
}

fn find_next_ports_row(state: &AppState, start: usize, direction: isize) -> Option<usize> {
    if direction == 0 {
        return None;
    }
    let len = state.visible_ports.len() as isize;
    let mut idx = start as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return None;
        }
        let next = idx as usize;
        if !state.is_ports_group_row(next) {
            return Some(next);
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
    if state.is_node_selectable_row(state.selected) {
        return;
    }
    if let Some(next) = find_next_node_row(state, state.selected, 1) {
        state.selected = next;
    } else if let Some(prev) = find_next_node_row(state, state.selected, -1) {
        state.selected = prev;
    }
}

fn find_next_node_row(state: &AppState, start: usize, direction: isize) -> Option<usize> {
    if direction == 0 {
        return None;
    }
    let len = state.visible_pids.len() as isize;
    let mut idx = start as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return None;
        }
        let next = idx as usize;
        if state.is_node_selectable_row(next) {
            return Some(next);
        }
    }
}
