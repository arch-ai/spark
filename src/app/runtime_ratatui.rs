//! Ratatui-based runtime for the Spark system manager

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sysinfo::{Disks, Pid, System};

use crate::app::input::{handle_key_event, handle_mouse_event, log_modal_inner_size};
use crate::app::{AppState, NodeTab, ViewMode};
use crate::system::{docker, node, ports, process};
use crate::ui::render_ratatui;

struct ProcessBuildResult {
    version: u64,
    source: Arc<Vec<process::ProcessEntry>>,
    process_cache: HashMap<Pid, process::ProcInfo>,
    rows_cache: Vec<process::TreeRow>,
    visible_pids: Vec<Pid>,
    identities: HashMap<u32, u64>,
}

struct NodeBuildResult {
    version: u64,
    source: Arc<node::NodeSnapshot>,
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
    source: Arc<Vec<docker::ContainerInfo>>,
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
    source: Arc<Vec<ports::PortInfo>>,
    ports_cache: Vec<ports::PortInfo>,
    ports_rows: Vec<ports::PortRow>,
    visible_ports: Vec<Pid>,
    visible_port_indices: Vec<Option<usize>>,
    visible_ports_container_ids: Vec<Option<String>>,
}

pub fn run_ratatui(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut system = System::new();
    system.refresh_cpu();
    system.refresh_memory();

    let docker_worker = docker::start_docker_stats_worker(Duration::from_secs(2));
    let docker_df_worker = docker::start_docker_df_worker(Duration::from_secs(10));
    let ports_worker = ports::start_ports_worker(Duration::from_secs(5));
    let process_worker = process::start_process_worker(Duration::from_secs(2));
    let node_worker = node::start_node_worker(Duration::from_secs(2));

    let mut state = AppState::new();
    let config_path = super::workspace_config::config_path();
    if let Err(error) = super::workspace_config::load(&mut state, &config_path) {
        state.workspace.config_error = Some(error.to_string());
        state.set_message(format!("Workspace configuration preserved: {error}"));
    }
    state.workspace.config_path = Some(config_path);
    let mut catalog = super::workspace_runtime::Catalog::default();
    update_system_snapshot(&mut state, &system);
    maybe_refresh_user_cache(&mut state);

    let tick_rate = Duration::from_millis(1000);
    let input_poll = Duration::from_millis(50);
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
    let mut docker_metadata: Arc<Vec<docker::ContainerInfo>> = Arc::new(Vec::new());
    let mut docker_view: Vec<docker::ContainerInfo> = Vec::new();
    let mut docker_dirty = true;
    let mut docker_snapshot = docker_worker.snapshot();
    let mut docker_df_snapshot = docker_df_worker.snapshot();
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
    let size = terminal
        .size()
        .unwrap_or_else(|_| terminal.size().unwrap_or_default());
    let mut term_width = size.width;
    let mut term_height = size.height;
    state.term_width = term_width;
    state.term_height = term_height;
    let mut mouse_capture_enabled = true;

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
                if key.kind != crossterm::event::KeyEventKind::Release
                    && key.code == crossterm::event::KeyCode::F(5)
                {
                    state.docker_memory.reconnect();
                }
                let prev_filter = state.active_filter().to_string();
                let prev_sort_by = state.sort_by;
                let prev_sort_order = state.sort_order;
                let prev_sorts = state.table_sorts;
                let prev_volume_scope = state.docker_volume_scope.clone();
                let prev_zoom = state.zoom;
                let prev_view = state.view_mode;
                let prev_log_open = state.log_output.is_some();
                let prev_list_open = state.docker_list_open;

                if handle_key_event(
                    key,
                    &mut state,
                    &mut system,
                    &pm2_view,
                    &pm2_rows,
                    &docker_view,
                    &ports_cache,
                ) {
                    break;
                }

                let filter_changed = state.active_filter() != prev_filter;
                let sort_changed = state.sort_by != prev_sort_by
                    || state.sort_order != prev_sort_order
                    || state.table_sorts != prev_sorts;
                let zoom_changed = state.zoom != prev_zoom;
                let view_changed = state.view_mode != prev_view;
                let modal_closed = (prev_log_open && state.log_output.is_none())
                    || (prev_list_open && !state.docker_list_open);

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
                        ViewMode::DockerEnv | ViewMode::Projects => {}
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
                if sort_changed || state.docker_volume_scope != prev_volume_scope {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                    docker_dirty = true;
                    docker_build_version = docker_build_version.wrapping_add(1);
                    ports_dirty = true;
                    ports_build_version = ports_build_version.wrapping_add(1);
                    node_dirty = true;
                    node_build_version = node_build_version.wrapping_add(1);
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
                let prev_sorts = state.table_sorts;
                let prev_sort = (state.sort_by, state.sort_order);
                let prev_log_open = state.log_output.is_some();
                let prev_list_open = state.docker_list_open;

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
                if prev_sorts != state.table_sorts || prev_sort != (state.sort_by, state.sort_order)
                {
                    process_dirty = true;
                    process_build_version = process_build_version.wrapping_add(1);
                    docker_dirty = true;
                    docker_build_version = docker_build_version.wrapping_add(1);
                    ports_dirty = true;
                    ports_build_version = ports_build_version.wrapping_add(1);
                    node_dirty = true;
                    node_build_version = node_build_version.wrapping_add(1);
                }
                let modal_closed = (prev_log_open && state.log_output.is_none())
                    || (prev_list_open && !state.docker_list_open);

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
        let workspace_active =
            state.view_mode == ViewMode::Projects || state.workspace.inspector.is_some();
        let docker_active = matches!(state.view_mode, ViewMode::Docker | ViewMode::DockerEnv);
        let ports_active = matches!(state.view_mode, ViewMode::Ports);
        let process_active = matches!(state.view_mode, ViewMode::Process);
        let node_active = matches!(state.view_mode, ViewMode::Node);

        let want_mouse_capture = !state.log_select_mode;
        if want_mouse_capture != mouse_capture_enabled {
            if want_mouse_capture {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
            }
            mouse_capture_enabled = want_mouse_capture;
        }

        docker_worker.set_paused(log_modal_open || !(docker_active || workspace_active));
        docker_df_worker.set_paused(log_modal_open || !(docker_active || workspace_active));
        ports_worker.set_paused(log_modal_open || !(ports_active || workspace_active));
        process_worker.set_paused(log_modal_open || !(process_active || workspace_active));
        node_worker.set_paused(log_modal_open || !(node_active || workspace_active));

        let metadata = docker_worker.snapshot();
        let metadata_changed = !Arc::ptr_eq(&docker_metadata, &metadata.data);
        if metadata_changed {
            docker_metadata = metadata.data.clone();
            state.docker_memory.sync_containers(&docker_metadata);
        }
        let memory_active = docker_active
            || state.view_mode == ViewMode::Projects
            || state.workspace.inspector.as_ref().is_some_and(|i| {
                matches!(
                    i.record.resource,
                    super::projects::Resource::Container(_) | super::projects::Resource::Project(_)
                )
            });
        let memory_changed = state.docker_memory.update(memory_active);
        if metadata_changed || memory_changed {
            docker_raw = Arc::new(
                docker_metadata
                    .iter()
                    .cloned()
                    .map(|mut container| {
                        container.memory = state.docker_memory.get(&container.id);
                        container
                    })
                    .collect(),
            );
            docker_dirty = true;
            needs_render = true;
        }

        // Periodic system refresh (paused while log modal is open)
        if last_tick.elapsed() >= tick_rate {
            if !log_modal_open {
                refresh_system(&mut system);
                update_system_snapshot(&mut state, &system);

                match state.view_mode {
                    ViewMode::Process | ViewMode::Projects => {}
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
        if state.docker_refresh_requested {
            state.docker_refresh_requested = false;
            docker_worker.refresh();
            docker_df_worker.refresh();
        }

        if state.refresh_requested {
            state.refresh_requested = false;
            process_worker.refresh();
            ports_worker.refresh();
            node_worker.refresh();
        }
        state.process_loaded = process_worker.is_loaded();
        state.ports_loaded = ports_worker.is_loaded();
        state.ports_error = ports_worker.error();
        if state.view_mode == ViewMode::Node && state.node_tab == NodeTab::Pm2 {
            let (pm2_area, _) = crate::ui::layout::node_tables(
                crate::ui::workspace::panes(
                    crate::ui::layout::main_area(ratatui::layout::Rect::new(
                        0,
                        0,
                        term_width,
                        term_height,
                    )),
                    state.workspace.inspector.is_some(),
                )
                .0,
                state.node_tab,
            );
            let capacity = pm2_area.height.saturating_sub(3) as usize;
            if state.pm2_selected < state.pm2_scroll {
                state.pm2_scroll = state.pm2_selected;
            } else if capacity > 0 && state.pm2_selected >= state.pm2_scroll + capacity {
                state.pm2_scroll = state.pm2_selected + 1 - capacity;
            }
        }
        if state.tick_spinner() {
            needs_render = true;
        }
        if state.tick_logo_at(Instant::now()) {
            needs_render = true;
        }

        while let Ok(result) = process_build_rx.try_recv() {
            if state.view_mode != ViewMode::Process {
                process_dirty = true;
                process_build_in_progress = false;
                continue;
            }
            if result.version != process_build_version {
                process_dirty = true;
                process_build_in_progress = false;
                continue;
            }
            let selected_pid = state.visible_pids.get(state.selected).copied();
            if let Some(selected) = selected_pid
                .and_then(|pid| result.visible_pids.iter().position(|next| *next == pid))
            {
                state.selected = selected;
            }
            state.process_identities = result.identities;
            process_cache = result.process_cache;
            rows_cache = result.rows_cache;
            state.visible_pids = result.visible_pids;
            clamp_selection(&mut state, rows_cache.len());
            process_dirty = !Arc::ptr_eq(&result.source, &process_raw);
            process_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = node_build_rx.try_recv() {
            if state.view_mode != ViewMode::Node {
                node_dirty = true;
                node_build_in_progress = false;
                continue;
            }
            if result.version != node_build_version {
                node_dirty = true;
                node_build_in_progress = false;
                continue;
            }
            let selected_pm2 = pm2_rows
                .get(state.pm2_selected)
                .and_then(|idx| pm2_view.get(*idx))
                .map(|proc| proc.pm_id);
            let selected_pid = state
                .visible_pids
                .get(state.selected)
                .copied()
                .filter(|pid| pid.as_u32() != 0);
            if let Some(next) = selected_pid
                .and_then(|pid| result.visible_pids.iter().position(|next| *next == pid))
            {
                state.selected = next;
            }
            if let Some(next) = selected_pm2.and_then(|pm_id| {
                result
                    .pm2_rows
                    .iter()
                    .position(|idx| result.pm2_view[*idx].pm_id == pm_id)
            }) {
                state.pm2_selected = next;
            }
            state.pm2_selected = state
                .pm2_selected
                .min(result.pm2_rows.len().saturating_sub(1));
            state.pm2_hover_row = None;
            node_view = result.node_view;
            node_rows = result.node_rows;
            pm2_view = result.pm2_view;
            pm2_rows = result.pm2_rows;
            state.pm2_available = result.pm2_available;
            state.visible_pids = result.visible_pids;
            state.visible_node_selectable = result.visible_node_selectable;
            clamp_selection(&mut state, node_rows.len());
            clamp_node_selection(&mut state);
            node_dirty = !Arc::ptr_eq(&result.source, &node_raw);
            node_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = docker_build_rx.try_recv() {
            if result.version != docker_build_version {
                docker_dirty = true;
                docker_build_in_progress = false;
                continue;
            }
            state.docker_selected_row = docker_selection_after_refresh(
                &state,
                &result.docker_rows,
                &result.visible_containers,
            );
            state.hover_row = None;
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
            docker_dirty = !Arc::ptr_eq(&result.source, &docker_raw);
            docker_build_in_progress = false;
            needs_render = true;
        }

        while let Ok(result) = ports_build_rx.try_recv() {
            if state.view_mode != ViewMode::Ports {
                ports_dirty = true;
                ports_build_in_progress = false;
                continue;
            }
            if result.version != ports_build_version {
                ports_dirty = true;
                ports_build_in_progress = false;
                continue;
            }
            let selected_port = ports_rows.get(state.selected).and_then(|row| match row {
                ports::PortRow::Item { index } => ports_cache.get(*index),
                _ => None,
            });
            if let Some(old) = selected_port {
                if let Some(next) = result.ports_rows.iter().position(|row| match row {
                    ports::PortRow::Item { index } => {
                        let next = &result.ports_cache[*index];
                        next.proto == old.proto
                            && next.port == old.port
                            && next.pid == old.pid
                            && next.container_id == old.container_id
                    }
                    _ => false,
                }) {
                    state.selected = next;
                }
            }
            state.visible_port_indices = result.visible_port_indices;
            state.hover_row = None;
            ports_cache = result.ports_cache;
            ports_rows = result.ports_rows;
            state.visible_ports = result.visible_ports;
            state.visible_ports_container_ids = result.visible_ports_container_ids;
            clamp_selection(&mut state, ports_rows.len());
            clamp_ports_selection(&mut state);
            ports_dirty = !Arc::ptr_eq(&result.source, &ports_raw);
            ports_build_in_progress = false;
            needs_render = true;
        }

        // Update data based on current view (paused while log modal is open)
        if !log_modal_open {
            match state.view_mode {
                ViewMode::Projects => {}
                ViewMode::Process => {
                    let new_snapshot = process_worker.snapshot();
                    if !Arc::ptr_eq(&new_snapshot, &process_raw) {
                        process_raw = new_snapshot;
                        process_dirty = true;
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
                            );
                            let rows_cache =
                                process::build_tree_rows(&process_cache, sort_by, sort_order, zoom);
                            let visible_pids = rows_cache.iter().map(|row| row.pid).collect();
                            let identities = entries
                                .iter()
                                .map(|entry| (entry.pid.as_u32(), entry.start_time))
                                .collect();
                            let _ = tx.send(ProcessBuildResult {
                                version,
                                source: Arc::clone(&entries),
                                process_cache,
                                identities,
                                rows_cache,
                                visible_pids,
                            });
                        });
                    }
                }
                ViewMode::Docker => {
                    let next_df = docker_df_worker.snapshot();
                    if !Arc::ptr_eq(&next_df, &docker_df_snapshot) {
                        state.docker_system_df = (*next_df.data).clone();
                        state.docker_df_error = next_df.error.clone();
                        state.docker_df_updated_at = next_df.updated_at;
                        docker_df_snapshot = next_df;
                        needs_render = true;
                    }
                    let next = docker_worker.snapshot();
                    if !Arc::ptr_eq(&next, &docker_snapshot) {
                        state.docker_error = next.error.clone();
                        state.docker_updated_at = next.updated_at;
                        docker_snapshot = next;
                        needs_render = true;
                    }

                    if docker_dirty && !docker_build_in_progress {
                        let snapshot = Arc::clone(&docker_raw);
                        let filter = state.docker_filter.clone();
                        let sort = state.sort_for(crate::app::sorting::SortTarget::Docker);
                        let scope = state.docker_volume_scope.clone();
                        let tx = docker_build_tx.clone();
                        let version = docker_build_version;
                        docker_build_in_progress = true;
                        docker_dirty = false;
                        std::thread::spawn(move || {
                            let mut docker_view = (*snapshot).clone();
                            docker::apply_container_filter(&mut docker_view, &filter);
                            if let Some((_, ids)) = scope {
                                docker_view
                                    .retain(|container| ids.iter().any(|id| id == &container.id));
                            }
                            let (grouped, rows) =
                                docker::group_containers_sorted(docker_view, Some(sort));
                            let docker_total = snapshot.len();
                            let docker_filtered_out = docker_total.saturating_sub(grouped.len());

                            let mut visible_containers = Vec::with_capacity(grouped.len());
                            let mut visible_container_names = Vec::with_capacity(grouped.len());
                            let mut visible_container_ports_public =
                                Vec::with_capacity(grouped.len());
                            let mut visible_container_ports_internal =
                                Vec::with_capacity(grouped.len());
                            let mut visible_container_group_name =
                                Vec::with_capacity(grouped.len());
                            let mut visible_container_group_path =
                                Vec::with_capacity(grouped.len());

                            for container in &grouped {
                                visible_containers.push(container.id.clone());
                                visible_container_names.push(container.name.clone());
                                visible_container_ports_public.push(container.port_public.clone());
                                visible_container_ports_internal
                                    .push(container.port_internal.clone());
                                visible_container_group_name.push(container.group_name.clone());
                                visible_container_group_path.push(
                                    container
                                        .group_path
                                        .clone()
                                        .unwrap_or_else(|| "-".to_string()),
                                );
                            }

                            let _ = tx.send(DockerBuildResult {
                                version,
                                source: Arc::clone(&snapshot),
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
                    }

                    if ports_dirty && !ports_build_in_progress {
                        let snapshot = Arc::clone(&ports_raw);
                        let filter = state.ports_filter.clone();
                        let sort = state.sort_for(crate::app::sorting::SortTarget::Ports);
                        let tx = ports_build_tx.clone();
                        let version = ports_build_version;
                        ports_build_in_progress = true;
                        ports_dirty = false;
                        std::thread::spawn(move || {
                            let mut ports_cache = (*snapshot).clone();
                            crate::util::apply_filter(&mut ports_cache, &filter);
                            crate::app::sorting::sort_ports(&mut ports_cache, sort);
                            let ports_rows = ports::group_ports(&ports_cache);

                            let mut visible_ports = Vec::with_capacity(ports_rows.len());
                            let mut visible_port_indices = Vec::with_capacity(ports_rows.len());
                            let mut visible_ports_container_ids =
                                Vec::with_capacity(ports_rows.len());
                            for row in &ports_rows {
                                match row {
                                    ports::PortRow::Group { .. } => {
                                        visible_port_indices.push(None);
                                        visible_ports.push(Pid::from_u32(0));
                                        visible_ports_container_ids.push(None);
                                    }
                                    ports::PortRow::Item { index, .. } => {
                                        let port = &ports_cache[*index];
                                        visible_port_indices.push(Some(*index));
                                        visible_ports.push(port.pid);
                                        visible_ports_container_ids.push(port.container_id.clone());
                                    }
                                }
                            }

                            let _ = tx.send(PortsBuildResult {
                                version,
                                source: Arc::clone(&snapshot),
                                ports_cache,
                                ports_rows,
                                visible_ports,
                                visible_port_indices,
                                visible_ports_container_ids,
                            });
                        });
                    }
                }
                ViewMode::Node => {
                    let new_snapshot = node_worker.snapshot();
                    if !Arc::ptr_eq(&new_snapshot, &node_raw) {
                        state.node_loaded = new_snapshot.loaded;
                        state.pm2_error = new_snapshot.pm2_error.clone();
                        state.pm2_loading = new_snapshot.pm2_loading;
                        node_raw = new_snapshot;
                        node_dirty = true;
                    }
                    if node_dirty && !node_build_in_progress {
                        let snapshot = Arc::clone(&node_raw);
                        let filter = state.node_filter.clone();
                        let native_sort = state.sort_for(crate::app::sorting::SortTarget::Node);
                        let pm2_sort = state.sort_for(crate::app::sorting::SortTarget::Pm2);
                        let tx = node_build_tx.clone();
                        let version = node_build_version;
                        node_build_in_progress = true;
                        node_dirty = false;
                        std::thread::spawn(move || {
                            let pm2_available = snapshot.pm2_available;
                            let mut pm2_view = snapshot.pm2_procs.clone();
                            crate::app::sorting::sort_pm2(&mut pm2_view, pm2_sort);
                            let pm2_rows = if pm2_available {
                                node::filter_pm2_processes(&pm2_view, &filter)
                            } else {
                                pm2_view.clear();
                                Vec::new()
                            };
                            let mut node_cache =
                                node::filter_node_processes(&snapshot.node_procs, &filter);
                            crate::app::sorting::sort_node(&mut node_cache, native_sort);
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
                                node_rows.push(node::NodeRow::Group {
                                    name: "Utilities".into(),
                                    count: node_view.len(),
                                });
                                node_rows.extend(node::group_node_processes(&node_view, 0));
                            } else {
                                node_rows =
                                    node::group_node_processes(&node_view[..utils_offset], 0);
                                if utils_offset < node_view.len() {
                                    node_rows.push(node::NodeRow::Group {
                                        name: "Utilities".into(),
                                        count: node_view.len() - utils_offset,
                                    });
                                    node_rows.extend(node::group_node_processes(
                                        &node_view[utils_offset..],
                                        utils_offset,
                                    ));
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
                                source: Arc::clone(&snapshot),
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

        if workspace_active && !log_modal_open {
            state.workspace.start_events();
            let processes = process_worker.snapshot();
            let docker = docker_worker.snapshot();
            let nodes = node_worker.snapshot();
            let port_snapshot = ports_worker.snapshot();
            let errors = [
                docker.error.clone(),
                nodes.pm2_error.clone(),
                ports_worker.error(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            state.workspace.catalog_error = (!errors.is_empty()).then(|| errors.join(" · "));
            needs_render |= catalog.update(
                &mut state,
                processes,
                docker.data.clone(),
                nodes,
                port_snapshot,
            );
        }
        needs_render |= state.workspace.drain();
        let navigation_view = state.view_mode;
        let navigation_pending = state.workspace.navigation.is_some();
        let owner_pending = state.workspace.owner_request.is_some();
        super::workspace_input::complete_navigation(&mut state, &pm2_view, &pm2_rows);
        if navigation_view != state.view_mode {
            process_dirty = true;
            docker_dirty = true;
            ports_dirty = true;
            node_dirty = true;
            terminal.clear()?;
            needs_render = true;
        }
        needs_render |= navigation_pending != state.workspace.navigation.is_some()
            || owner_pending != state.workspace.owner_request.is_some();
        if let Some(copy) = state.workspace.clipboard.take() {
            use std::io::Write;
            terminal
                .backend_mut()
                .write_all(super::workspace::clipboard_escape(&copy).as_bytes())?;
            terminal.backend_mut().flush()?;
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
                ViewMode::Projects => state
                    .workspace
                    .visible_projects(state.sort_for(crate::app::sorting::SortTarget::Projects))
                    .len(),
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

    if state.workspace.config_error.is_none() {
        if let Some(path) = &state.workspace.config_path {
            super::workspace_config::save(path, &super::workspace_config::encode(&state))?;
        }
    }
    Ok(())
}

/// Calculate visible height for table rows based on view mode and terminal height
fn adjust_visible_height(state: &AppState, height: u16) -> usize {
    let full =
        crate::ui::layout::main_area(ratatui::layout::Rect::new(0, 0, state.term_width, height));
    let area = crate::ui::workspace::panes(full, state.workspace.inspector.is_some()).0;
    let table = match state.view_mode {
        ViewMode::Projects => crate::ui::workspace::project_layout(area)[1],
        ViewMode::Process => crate::ui::layout::process_layout(area)[4],
        ViewMode::Docker => crate::ui::layout::docker_layout(area)[4],
        ViewMode::Ports => crate::ui::layout::resource_layout(area)[3],
        ViewMode::Node => crate::ui::layout::node_tables(area, state.node_tab).1,
        ViewMode::DockerEnv => return 0,
    };
    table.height.saturating_sub(3) as usize
}

fn refresh_system(system: &mut System) {
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
            state
                .user_cache
                .insert(user.id().clone(), user.name().to_string());
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

fn docker_selection_after_refresh(
    state: &AppState,
    rows: &[docker::DockerRow],
    ids: &[String],
) -> usize {
    let old_row = state.docker_rows.get(state.docker_selected_row);
    let found = rows.iter().position(|row| match (old_row, row) {
        (
            Some(docker::DockerRow::Item { index: old, .. }),
            docker::DockerRow::Item { index: new, .. },
        ) => state
            .visible_containers
            .get(*old)
            .zip(ids.get(*new))
            .is_some_and(|(a, b)| a == b),
        (
            Some(docker::DockerRow::Group {
                name: old_name,
                path: old_path,
                ..
            }),
            docker::DockerRow::Group { name, path, .. },
        ) => old_name == name && old_path == path,
        _ => false,
    });
    found.unwrap_or_else(|| {
        let fallback = state.docker_selected_row.min(rows.len().saturating_sub(1));
        (0..=fallback)
            .rev()
            .find(|&index| {
                rows.get(index)
                    .is_some_and(|row| !matches!(row, docker::DockerRow::Separator))
            })
            .unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_scroll_capacity_matches_rendered_memory_rows() {
        let state = AppState::new();
        for height in [16, 20, 24, 36] {
            assert_eq!(
                adjust_visible_height(&state, height),
                crate::ui::layout::process_table(
                    crate::ui::layout::main_area(ratatui::layout::Rect::new(
                        0,
                        0,
                        state.term_width,
                        height
                    ))
                    .width,
                    height
                )
                .height
                .saturating_sub(3) as usize
            );
        }
    }

    fn item(index: usize) -> docker::DockerRow {
        docker::DockerRow::Item {
            index,
            prefix: String::new(),
        }
    }

    #[test]
    fn docker_selection_follows_container_identity_across_reorder() {
        let mut state = AppState::new();
        state.docker_rows = vec![item(0), item(1)];
        state.visible_containers = vec!["alpha".into(), "beta".into()];
        state.docker_selected_row = 1;
        assert_eq!(
            docker_selection_after_refresh(
                &state,
                &[item(0), item(1)],
                &["beta".into(), "alpha".into()]
            ),
            0
        );
        assert_eq!(
            docker_selection_after_refresh(
                &state,
                &[item(0), docker::DockerRow::Separator],
                &["alpha".into()]
            ),
            0
        );
        assert_eq!(docker_selection_after_refresh(&state, &[], &[]), 0);
    }

    #[test]
    fn docker_scroll_capacity_matches_rendered_rows() {
        let state = AppState::new();
        let mut state = state;
        state.view_mode = ViewMode::Docker;
        for height in [16, 20, 24, 36] {
            assert_eq!(
                adjust_visible_height(&state, height),
                crate::ui::layout::docker_table(
                    crate::ui::layout::main_area(ratatui::layout::Rect::new(
                        0,
                        0,
                        state.term_width,
                        height
                    ))
                    .width,
                    height
                )
                .height
                .saturating_sub(3) as usize
            );
        }
    }
}
