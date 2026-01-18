//! Ratatui-based runtime for the Spark system manager

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sysinfo::{Disks, Pid, System};

use crate::app::input::{handle_key_event, handle_mouse_event, log_modal_inner_size};
use crate::app::{AppState, ViewMode};
use crate::system::{docker, node, ports, process};
use crate::ui::render_ratatui;

struct ProcessBuildResult {
    version: u64,
    process_cache: HashMap<Pid, process::ProcInfo>,
    rows_cache: Vec<process::TreeRow>,
    visible_pids: Vec<Pid>,
}

struct NodeBuildResult {
    version: u64,
    node_view: Vec<node::NodeProcessInfo>,
    node_rows: Vec<node::NodeRow>,
    pm2_view: Vec<node::Pm2Process>,
    pm2_rows: Vec<usize>,
    visible_pids: Vec<Pid>,
    visible_node_selectable: Vec<bool>,
    pm2_available: bool,
}

struct DockerBuildResult {
    version: u64,
    docker_view: Vec<docker::ContainerInfo>,
    docker_rows: Vec<docker::DockerRow>,
    visible_containers: Vec<String>,
    visible_container_names: Vec<String>,
    visible_container_ports_public: Vec<std::borrow::Cow<'static, str>>,
    visible_container_ports_internal: Vec<std::borrow::Cow<'static, str>>,
    visible_container_group_name: Vec<std::borrow::Cow<'static, str>>,
    visible_container_group_path: Vec<String>,
    docker_total: usize,
    docker_filtered_out: usize,
}

struct PortsBuildResult {
    version: u64,
    ports_cache: Vec<ports::PortInfo>,
    ports_rows: Vec<ports::PortRow>,
    visible_ports: Vec<Pid>,
    visible_ports_container_ids: Vec<Option<String>>,
}

pub fn run_ratatui(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut system = System::new_all();
    system.refresh_all();

    let docker_worker = docker::start_docker_stats_worker(Duration::from_secs(2));
    let docker_df_worker = docker::start_docker_df_worker(Duration::from_secs(10));
    let ports_worker = ports::start_ports_worker(Duration::from_secs(5));
    let process_worker = process::start_process_worker(Duration::from_secs(2));
    let node_worker = node::start_node_worker(Duration::from_secs(2));

    let mut state = AppState::new();
    update_system_snapshot(&mut state, &system);
    maybe_refresh_user_cache(&mut state);

    let tick_rate = Duration::from_millis(1000);
    let input_poll = Duration::from_millis(16);
    let mut last_tick = Instant::now();
    let mut needs_render = true;

    // Process view cache
    let mut process_cache: HashMap<Pid, process::ProcInfo> = HashMap::new();
    let mut rows_cache: Vec<process::TreeRow> = Vec::new();
    let mut process_dirty = true;
    let mut process_raw: Arc<Vec<process::ProcessEntry>> = Arc::new(Vec::new());
    let (process_build_tx, process_build_rx) = mpsc::channel::<ProcessBuildResult>();
    let mut process_build_version: u64 = 0;
    let mut process_build_in_progress = false;

    // Docker view cache - docker_raw uses Arc for efficient snapshot without cloning
    let mut docker_raw: Arc<Vec<docker::ContainerInfo>> = Arc::new(Vec::new());
    let mut docker_view: Vec<docker::ContainerInfo> = Vec::new();
    let mut docker_dirty = true;
    let mut last_docker_pull = Instant::now() - Duration::from_secs(60);
    let mut last_docker_df_pull = Instant::now();
    let (docker_build_tx, docker_build_rx) = mpsc::channel::<DockerBuildResult>();
    let mut docker_build_version: u64 = 0;
    let mut docker_build_in_progress = false;

    // Ports view cache - uses background worker, so just need mutable state for filtering/grouping
    let mut ports_raw: Arc<Vec<ports::PortInfo>> = Arc::new(Vec::new());
    let mut ports_cache: Vec<ports::PortInfo> = Vec::new();
    let mut ports_rows: Vec<ports::PortRow> = Vec::new();
    let mut ports_dirty = true;
    let (ports_build_tx, ports_build_rx) = mpsc::channel::<PortsBuildResult>();
    let mut ports_build_version: u64 = 0;
    let mut ports_build_in_progress = false;

    // Node view cache
    let mut node_view: Vec<node::NodeProcessInfo> = Vec::new();
    let mut node_rows: Vec<node::NodeRow> = Vec::new();
    let mut pm2_view: Vec<node::Pm2Process> = Vec::new();
    let mut pm2_rows: Vec<usize> = Vec::new();
    let mut node_dirty = true;
    let mut node_raw: Arc<node::NodeSnapshot> = Arc::new(node::NodeSnapshot::default());
    let (node_build_tx, node_build_rx) = mpsc::channel::<NodeBuildResult>();
    let mut node_build_version: u64 = 0;
    let mut node_build_in_progress = false;

    // Cache terminal size to avoid syscalls on every mouse event
    let size = terminal.size().unwrap_or_else(|_| terminal.size().unwrap_or_default());
    let mut term_width = size.width;
    let mut term_height = size.height;
    state.term_width = term_width;
    state.term_height = term_height;

    loop {
        // Handle input events
        if event::poll(input_poll)? {
            let ev = event::read()?;

            if let Event::Resize(width, height) = ev {
                term_width = width;
                term_height = height;
                state.term_width = width;
                state.term_height = height;
                state.log_wrap_width = 0;
                state.log_lines.clear();
                state.log_line_count = 0;
                terminal.clear()?;
                needs_render = true;
            }

            if let Event::Key(key) = ev {
                let prev_filter = state.active_filter().to_string();
                let prev_sort_by = state.sort_by;
                let prev_sort_order = state.sort_order;
                let prev_zoom = state.zoom;
                let prev_view = state.view_mode;
                let prev_log_open = state.log_output.is_some();
                let prev_prune_open = state.prune_output.is_some();

                if handle_key_event(key, &mut state, &mut system, &pm2_view, &pm2_rows) {
                    break;
                }

                let filter_changed = state.active_filter() != prev_filter;
                let sort_changed = state.sort_by != prev_sort_by || state.sort_order != prev_sort_order;
                let zoom_changed = state.zoom != prev_zoom;
                let view_changed = state.view_mode != prev_view;
                let modal_closed = (prev_log_open && state.log_output.is_none())
                    || (prev_prune_open && state.prune_output.is_none());

                if filter_changed {
                    match state.view_mode {
                        ViewMode::Process => {
                            process_dirty = true;
                            process_build_version = process_build_version.wrapping_add(1);
                        }
                        ViewMode::Docker => {
                            docker_dirty = true;
                            docker_build_version = docker_build_version.wrapping_add(1);
                        }
                        ViewMode::DockerEnv => {}
                        ViewMode::Ports => {
                            ports_dirty = true;
                            ports_build_version = ports_build_version.wrapping_add(1);
                        }
                        ViewMode::Node => {
                            node_dirty = true;
                            node_build_version = node_build_version.wrapping_add(1);
                        }
                    }
                }
                if sort_changed {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                    docker_dirty = true;
                    docker_build_version = docker_build_version.wrapping_add(1);
                }
                if zoom_changed {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                }
                if view_changed {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                    docker_dirty = true;
                    docker_build_version = docker_build_version.wrapping_add(1);
                    node_dirty = true;
                    node_build_version = node_build_version.wrapping_add(1);
                    ports_dirty = true;
                    ports_build_version = ports_build_version.wrapping_add(1);
                }

                // Force terminal clear when modal closes to remove artifacts
                if modal_closed {
                    terminal.clear()?;
                    docker_dirty = true;
                }

                needs_render = true;
            }

            if let Event::Mouse(mouse) = ev {
                let prev_view = state.view_mode;
                let prev_log_open = state.log_output.is_some();
                let prev_prune_open = state.prune_output.is_some();

                let mouse_needs_render = handle_mouse_event(
                    mouse,
                    &mut state,
                    &docker_view,
                    &ports_cache,
                    &pm2_view,
                    &pm2_rows,
                    term_width,
                    term_height,
                );
                let view_changed = state.view_mode != prev_view;
                let modal_closed = (prev_log_open && state.log_output.is_none())
                    || (prev_prune_open && state.prune_output.is_none());

                if view_changed {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                    docker_dirty = true;
                    docker_build_version = docker_build_version.wrapping_add(1);
                    node_dirty = true;
                    node_build_version = node_build_version.wrapping_add(1);
                    ports_dirty = true;
                    ports_build_version = ports_build_version.wrapping_add(1);
                }

                // Force terminal clear when modal closes to remove artifacts
                if modal_closed {
                    terminal.clear()?;
                    docker_dirty = true;
                    needs_render = true;
                }

                if mouse_needs_render || view_changed {
                    needs_render = true;
                }
            }
        }

        let log_modal_open = state.log_output.is_some() || state.log_in_progress.is_some();
        let docker_active = matches!(state.view_mode, ViewMode::Docker | ViewMode::DockerEnv);
        let ports_active = matches!(state.view_mode, ViewMode::Ports);
        let process_active = matches!(state.view_mode, ViewMode::Process);
        let node_active = matches!(state.view_mode, ViewMode::Node);

        docker_worker.set_paused(log_modal_open || !docker_active);
        docker_df_worker.set_paused(log_modal_open || !docker_active);
        ports_worker.set_paused(log_modal_open || !ports_active);
        process_worker.set_paused(log_modal_open || !process_active);
        node_worker.set_paused(log_modal_open || !node_active);

        // Periodic system refresh (paused while log modal is open)
        if last_tick.elapsed() >= tick_rate {
            if !log_modal_open {
                refresh_system(&mut system);
                update_system_snapshot(&mut state, &system);

                match state.view_mode {
                    ViewMode::Process => {}
                    ViewMode::Docker | ViewMode::DockerEnv => {}
                    ViewMode::Ports => {}
                    ViewMode::Node => {}
                }
                needs_render = true;
            }
            last_tick = Instant::now();
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

        while let Ok(result) = process_build_rx.try_recv() {
            if result.version != process_build_version {
                process_build_in_progress = false;
                continue;
            }
            process_cache = result.process_cache;
            rows_cache = result.rows_cache;
            state.visible_pids = result.visible_pids;
            clamp_selection(&mut state, rows_cache.len());
            process_dirty = false;
            process_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = node_build_rx.try_recv() {
            if result.version != node_build_version {
                node_build_in_progress = false;
                continue;
            }
            node_view = result.node_view;
            node_rows = result.node_rows;
            pm2_view = result.pm2_view;
            pm2_rows = result.pm2_rows;
            state.pm2_available = result.pm2_available;
            state.visible_pids = result.visible_pids;
            state.visible_node_selectable = result.visible_node_selectable;
            clamp_selection(&mut state, node_rows.len());
            clamp_node_selection(&mut state);
            node_dirty = false;
            node_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = docker_build_rx.try_recv() {
            if result.version != docker_build_version {
                docker_build_in_progress = false;
                continue;
            }
            docker_view = result.docker_view;
            state.docker_rows = result.docker_rows;
            state.visible_containers = result.visible_containers;
            state.visible_container_names = result.visible_container_names;
            state.visible_container_ports_public = result.visible_container_ports_public;
            state.visible_container_ports_internal = result.visible_container_ports_internal;
            state.visible_container_group_name = result.visible_container_group_name;
            state.visible_container_group_path = result.visible_container_group_path;
            state.docker_total = result.docker_total;
            state.docker_filtered_out = result.docker_filtered_out;
            clamp_docker_selection(&mut state);
            docker_dirty = false;
            docker_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = ports_build_rx.try_recv() {
            if result.version != ports_build_version {
                ports_build_in_progress = false;
                continue;
            }
            ports_cache = result.ports_cache;
            ports_rows = result.ports_rows;
            state.visible_ports = result.visible_ports;
            state.visible_ports_container_ids = result.visible_ports_container_ids;
            clamp_selection(&mut state, ports_rows.len());
            clamp_ports_selection(&mut state);
            ports_dirty = false;
            ports_build_in_progress = false;
            needs_render = true;
        }

        if state.log_output.is_some()
            && state.log_source.is_some()
            && !state.log_refresh_in_progress
            && state.log_last_scroll.elapsed() >= Duration::from_millis(400)
            && state.log_last_refresh.elapsed() >= Duration::from_secs(1)
        {
            let title = state
                .log_output
                .as_ref()
                .map(|output| output.title.clone())
                .unwrap_or_else(|| "Logs".to_string());
            if let Some(source) = state.log_source.clone() {
                match source {
                    crate::app::state::LogSource::Process { pid } => {
                        crate::app::actions::start_log_refresh(&mut state, title, move || {
                            process::load_process_logs(pid)
                        });
                    }
                    crate::app::state::LogSource::Pm2 { pm_id } => {
                        crate::app::actions::start_log_refresh(&mut state, title, move || {
                            node::load_pm2_logs(pm_id)
                        });
                    }
                    crate::app::state::LogSource::Docker { container_id } => {
                        crate::app::actions::start_log_refresh(&mut state, title, move || {
                            docker::load_container_logs(&container_id)
                        });
                    }
                }
            }
        }

        // Update data based on current view (paused while log modal is open)
        if !log_modal_open {
            match state.view_mode {
            ViewMode::Process => {
                let new_snapshot = process_worker.snapshot();
                if !Arc::ptr_eq(&new_snapshot, &process_raw) {
                    process_raw = new_snapshot;
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                }
                if process_dirty && !process_build_in_progress {
                    maybe_refresh_user_cache(&mut state);
                    let entries = Arc::clone(&process_raw);
                    let filter = state.process_filter.clone();
                    let user_cache = state.user_cache.clone();
                    let sort_by = state.sort_by;
                    let sort_order = state.sort_order;
                    let zoom = state.zoom;
                    let tx = process_build_tx.clone();
                    let version = process_build_version;
                    process_build_in_progress = true;
                    process_dirty = false;
                    std::thread::spawn(move || {
                        let process_cache = process::collect_processes_from_entries(
                            &entries,
                            &filter,
                            &user_cache,
                            zoom,
                        );
                        let rows_cache = process::build_tree_rows(
                            &process_cache,
                            sort_by,
                            sort_order,
                            zoom,
                        );
                        let visible_pids = rows_cache.iter().map(|row| row.pid).collect();
                        let _ = tx.send(ProcessBuildResult {
                            version,
                            process_cache,
                            rows_cache,
                            visible_pids,
                        });
                    });
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
                    docker_build_version = docker_build_version.wrapping_add(1);
                    last_docker_pull = Instant::now();

                    if state.update_pending_with_containers(&docker_raw) {
                        needs_render = true;
                    }
                }

                if docker_dirty && !docker_build_in_progress {
                    let snapshot = Arc::clone(&docker_raw);
                    let filter = state.docker_filter.clone();
                    let tx = docker_build_tx.clone();
                    let version = docker_build_version;
                    docker_build_in_progress = true;
                    docker_dirty = false;
                    std::thread::spawn(move || {
                        let mut docker_view = (*snapshot).clone();
                        docker::apply_container_filter(&mut docker_view, &filter);
                    let (grouped, rows) = docker::group_containers(docker_view);
                        let docker_total = snapshot.len();
                        let docker_filtered_out = docker_total.saturating_sub(grouped.len());

                        let mut visible_containers = Vec::with_capacity(grouped.len());
                        let mut visible_container_names = Vec::with_capacity(grouped.len());
                        let mut visible_container_ports_public = Vec::with_capacity(grouped.len());
                        let mut visible_container_ports_internal = Vec::with_capacity(grouped.len());
                        let mut visible_container_group_name = Vec::with_capacity(grouped.len());
                        let mut visible_container_group_path = Vec::with_capacity(grouped.len());

                        for container in &grouped {
                            visible_containers.push(container.id.clone());
                            visible_container_names.push(container.name.clone());
                            visible_container_ports_public.push(container.port_public.clone());
                            visible_container_ports_internal.push(container.port_internal.clone());
                            visible_container_group_name.push(container.group_name.clone());
                            visible_container_group_path.push(
                                container.group_path.clone().unwrap_or_else(|| "-".to_string())
                            );
                        }

                        let _ = tx.send(DockerBuildResult {
                            version,
                            docker_view: grouped,
                            docker_rows: rows,
                            visible_containers,
                            visible_container_names,
                            visible_container_ports_public,
                            visible_container_ports_internal,
                            visible_container_group_name,
                            visible_container_group_path,
                            docker_total,
                            docker_filtered_out,
                        });
                    });
                }
            }
            ViewMode::DockerEnv => {}
            ViewMode::Ports => {
                // Snapshot ports from background worker (non-blocking)
                let new_ports = ports_worker.snapshot();

                // Only update if data changed (pointer comparison)
                if !Arc::ptr_eq(&new_ports, &ports_raw) {
                    ports_raw = new_ports;
                    ports_dirty = true;
                    ports_build_version = ports_build_version.wrapping_add(1);
                }

                if ports_dirty && !ports_build_in_progress {
                    let snapshot = Arc::clone(&ports_raw);
                    let filter = state.ports_filter.clone();
                    let tx = ports_build_tx.clone();
                    let version = ports_build_version;
                    ports_build_in_progress = true;
                    ports_dirty = false;
                    std::thread::spawn(move || {
                        let mut ports_cache = (*snapshot).clone();
                        crate::util::apply_filter(&mut ports_cache, &filter);
                        let ports_rows = ports::group_ports(&ports_cache);

                        let mut visible_ports = Vec::with_capacity(ports_rows.len());
                        let mut visible_ports_container_ids = Vec::with_capacity(ports_rows.len());
                        for row in &ports_rows {
                            match row {
                                ports::PortRow::Group { .. } => {
                                    visible_ports.push(Pid::from_u32(0));
                                    visible_ports_container_ids.push(None);
                                }
                                ports::PortRow::Item { index, .. } => {
                                    let port = &ports_cache[*index];
                                    visible_ports.push(port.pid);
                                    visible_ports_container_ids.push(port.container_id.clone());
                                }
                            }
                        }

                        let _ = tx.send(PortsBuildResult {
                            version,
                            ports_cache,
                            ports_rows,
                            visible_ports,
                            visible_ports_container_ids,
                        });
                    });
                }
            }
            ViewMode::Node => {
                let new_snapshot = node_worker.snapshot();
                if !Arc::ptr_eq(&new_snapshot, &node_raw) {
                    node_raw = new_snapshot;
                    node_dirty = true;
                    node_build_version = node_build_version.wrapping_add(1);
                }
                if node_dirty && !node_build_in_progress {
                    let snapshot = Arc::clone(&node_raw);
                    let filter = state.node_filter.clone();
                    let tx = node_build_tx.clone();
                    let version = node_build_version;
                    node_build_in_progress = true;
                    node_dirty = false;
                    std::thread::spawn(move || {
                        let pm2_available = snapshot.pm2_available;
                        let mut pm2_view = snapshot.pm2_procs.clone();
                        let pm2_rows = if pm2_available {
                            node::filter_pm2_processes(&pm2_view, &filter)
                        } else {
                            pm2_view.clear();
                            Vec::new()
                        };
                        let node_cache = node::filter_node_processes(&snapshot.node_procs, &filter);
                        let mut node_main = Vec::new();
                        let mut node_utils = Vec::new();
                        for proc in node_cache {
                            if pm2_available && proc.pm2.is_some() {
                                continue;
                            }
                            if node::is_node_util(&proc) {
                                node_utils.push(proc);
                            } else {
                                node_main.push(proc);
                            }
                        }

                        let utils_offset = node_main.len();
                        let mut node_view = node_main;
                        node_view.extend(node_utils);
                        let mut node_rows = Vec::new();
                        if node_view.is_empty() {
                            node_rows.clear();
                        } else if utils_offset == 0 {
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

                        let mut visible_pids = Vec::with_capacity(node_rows.len());
                        let mut visible_node_selectable = Vec::with_capacity(node_rows.len());
                        for row in &node_rows {
                            match row {
                                node::NodeRow::Item { index } => {
                                    let proc = &node_view[*index];
                                    visible_pids.push(proc.pid);
                                    visible_node_selectable.push(true);
                                }
                                _ => {
                                    visible_pids.push(Pid::from_u32(0));
                                    visible_node_selectable.push(false);
                                }
                            }
                        }

                        let _ = tx.send(NodeBuildResult {
                            version,
                            node_view,
                            node_rows,
                            pm2_view,
                            pm2_rows,
                            visible_pids,
                            visible_node_selectable,
                            pm2_available,
                        });
                    });
                }
            }
            }
        }

        // Render using ratatui
        if needs_render {
            // Update cached terminal size
            let size = terminal.size()?;
            term_width = size.width;
            term_height = size.height;
            state.term_width = term_width;
            state.term_height = term_height;
            let visible_height = adjust_visible_height(&state, size.height);
            let total = match state.view_mode {
                ViewMode::Process => rows_cache.len(),
                ViewMode::Docker => state.docker_rows.len(),
                ViewMode::Ports => ports_rows.len(),
                ViewMode::Node => node_rows.len(),
                ViewMode::DockerEnv => 0,
            };
            state.adjust_scroll(visible_height, total);
            if state.log_output.is_some() {
                if let Some((inner_w, _inner_h)) = log_modal_inner_size(&state) {
                    state.ensure_log_lines(inner_w);
                }
            }

            terminal.draw(|frame| {
                render_ratatui(
                    frame,
                    &state,
                    &process_cache,
                    &rows_cache,
                    &docker_view,
                    &state.docker_rows,
                    &ports_cache,
                    &ports_rows,
                    &node_view,
                    &node_rows,
                    &pm2_view,
                    &pm2_rows,
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
        ViewMode::Ports => 11,
        ViewMode::Node => {
            if state.pm2_available {
                let table_top = 7u16;
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
                        node_height = node_height.saturating_add(deficit);
                    }
                }
                return node_height.saturating_sub(2) as usize;
            }
            11
        }
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

    if state.last_disk_refresh.elapsed() >= Duration::from_secs(5) {
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
        state.last_disk_refresh = Instant::now();
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
