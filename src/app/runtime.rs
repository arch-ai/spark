use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use sysinfo::{Pid, System, Users};

use crate::app::input::handle_key_event;
use crate::app::{AppState, ViewMode};
use crate::system::{docker, ports, process};
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
    let mut ports_dirty = true;

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
                    ui::render_docker_envs(
                        stdout,
                        &state,
                        &state.docker_env_vars,
                        state.docker_env_name.as_deref(),
                        &state.docker_env_compose_name,
                        &state.docker_env_compose_path,
                        &state.docker_env_port_public,
                        &state.docker_env_port_internal,
                        state.docker_env_selected,
                    )?;
                    needs_render = false;
                }
            }
            ViewMode::Ports => {
                if ports_dirty {
                    ports_cache = ports::collect_ports(&system);
                    crate::util::apply_filter(&mut ports_cache, &state.ports_filter);
                    clamp_selection(&mut state, ports_cache.len());
                    state.visible_ports = ports_cache.iter().map(|row| row.pid).collect();
                    state.visible_ports_container_ids = ports_cache
                        .iter()
                        .map(|row| row.container_id.clone())
                        .collect();
                    state.visible_pids.clear();
                    state.visible_containers.clear();
                    ports_dirty = false;
                    needs_render = true;
                }

                if needs_render {
                    ui::render_ports(stdout, &state, &ports_cache)?;
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
