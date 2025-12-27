use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use sysinfo::{Pid, System, Users, Uid};

use crate::system::docker;
use crate::system::ports;
use crate::system::process;
use crate::ui;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Process,
    Docker,
    DockerEnv,
    Ports,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SortBy {
    Cpu,
    Memory,
    Name,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub fn toggle(self) -> Self {
        match self {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        }
    }
}

pub struct AppState {
    pub input_mode: InputMode,
    pub process_filter: String,
    pub docker_filter: String,
    pub ports_filter: String,
    pub sort_by: SortBy,
    pub sort_order: SortOrder,
    pub zoom: bool,
    pub view_mode: ViewMode,
    pub selected: usize,
    pub message: Option<String>,
    pub message_until: Option<Instant>,
    pub visible_pids: Vec<Pid>,
    pub visible_containers: Vec<String>,
    pub visible_container_names: Vec<String>,
    pub visible_container_ports_public: Vec<String>,
    pub visible_container_ports_internal: Vec<String>,
    pub visible_container_group_name: Vec<String>,
    pub visible_container_group_path: Vec<String>,
    pub visible_ports: Vec<Pid>,
    pub visible_ports_container_ids: Vec<Option<String>>,
    pub container_cache: HashMap<String, String>,
    pub container_last_refresh: Instant,
    pub user_cache: HashMap<Uid, String>,
    pub user_last_refresh: Instant,
    pub docker_filtered_out: usize,
    pub docker_total: usize,
    pub docker_env_vars: Vec<String>,
    pub docker_env_name: Option<String>,
    pub docker_env_compose_name: String,
    pub docker_env_compose_path: String,
    pub docker_env_port_public: String,
    pub docker_env_port_internal: String,
    pub docker_env_selected: usize,
    pub cpu_usage: f32,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl AppState {
    fn new() -> Self {
        Self {
            input_mode: InputMode::Normal,
            process_filter: String::new(),
            docker_filter: String::new(),
            ports_filter: String::new(),
            sort_by: SortBy::Memory,
            sort_order: SortOrder::Desc,
            zoom: false,
            view_mode: ViewMode::Process,
            selected: 0,
            message: None,
            message_until: None,
            visible_pids: Vec::new(),
            visible_containers: Vec::new(),
            visible_container_names: Vec::new(),
            visible_container_ports_public: Vec::new(),
            visible_container_ports_internal: Vec::new(),
            visible_container_group_name: Vec::new(),
            visible_container_group_path: Vec::new(),
            visible_ports: Vec::new(),
            visible_ports_container_ids: Vec::new(),
            container_cache: HashMap::new(),
            container_last_refresh: Instant::now() - Duration::from_secs(60),
            user_cache: HashMap::new(),
            user_last_refresh: Instant::now() - Duration::from_secs(60),
            docker_filtered_out: 0,
            docker_total: 0,
            docker_env_vars: Vec::new(),
            docker_env_name: None,
            docker_env_compose_name: "-".to_string(),
            docker_env_compose_path: "-".to_string(),
            docker_env_port_public: "-".to_string(),
            docker_env_port_internal: "-".to_string(),
            docker_env_selected: 0,
            cpu_usage: 0.0,
            mem_total: 0,
            mem_available: 0,
            swap_total: 0,
            swap_used: 0,
        }
    }

    fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.message_until = Some(Instant::now() + Duration::from_secs(3));
    }

    fn clear_expired_message(&mut self) -> bool {
        if let Some(deadline) = self.message_until {
            if Instant::now() >= deadline {
                self.message = None;
                self.message_until = None;
                return true;
            }
        }
        false
    }

    fn toggle_sort(&mut self, sort_by: SortBy) {
        if self.sort_by == sort_by {
            self.sort_order = self.sort_order.toggle();
        } else {
            self.sort_by = sort_by;
            self.sort_order = SortOrder::Desc;
        }
    }

    fn active_filter(&self) -> &str {
        match self.view_mode {
            ViewMode::Process => &self.process_filter,
            ViewMode::Docker | ViewMode::DockerEnv => &self.docker_filter,
            ViewMode::Ports => &self.ports_filter,
        }
    }

    fn active_filter_mut(&mut self) -> &mut String {
        match self.view_mode {
            ViewMode::Process => &mut self.process_filter,
            ViewMode::Docker | ViewMode::DockerEnv => &mut self.docker_filter,
            ViewMode::Ports => &mut self.ports_filter,
        }
    }
}

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
            process_dirty = true;
            if state.view_mode == ViewMode::Ports {
                ports_dirty = true;
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
                    state.visible_containers =
                        docker_view.iter().map(|container| container.id.clone()).collect();
                    state.visible_container_names =
                        docker_view.iter().map(|container| container.name.clone()).collect();
                    state.visible_container_ports_public = docker_view
                        .iter()
                        .map(|container| container.port_public.clone())
                        .collect();
                    state.visible_container_ports_internal = docker_view
                        .iter()
                        .map(|container| container.port_internal.clone())
                        .collect();
                    state.visible_container_group_name = docker_view
                        .iter()
                        .map(|container| container.group_name.clone())
                        .collect();
                    state.visible_container_group_path = docker_view
                        .iter()
                        .map(|container| {
                            container
                                .group_path
                                .clone()
                                .unwrap_or_else(|| "-".to_string())
                        })
                        .collect();
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
                    if !state.ports_filter.is_empty() {
                        let filter = state.ports_filter.to_lowercase();
                        ports_cache.retain(|row| {
                            row.proto.to_lowercase().contains(&filter)
                                || row.port.to_string().contains(&filter)
                                || row.pid.to_string().contains(&filter)
                                || row.name.to_lowercase().contains(&filter)
                                || row.exe_path.to_lowercase().contains(&filter)
                        });
                    }
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

fn handle_key_event(key: KeyEvent, state: &mut AppState, system: &mut System) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if state.view_mode == ViewMode::DockerEnv {
        return handle_docker_env_mode(key, state);
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
    };

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
            state.view_mode = match state.view_mode {
                ViewMode::Process => ViewMode::Docker,
                ViewMode::Docker => ViewMode::Process,
                ViewMode::DockerEnv => ViewMode::Docker,
                ViewMode::Ports => ViewMode::Docker,
            };
            state.selected = 0;
            let label = match state.view_mode {
                ViewMode::Process => "Process",
                ViewMode::Docker => "Docker",
                ViewMode::DockerEnv => "Docker Env",
                ViewMode::Ports => "Ports",
            };
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('p') => {
            state.view_mode = match state.view_mode {
                ViewMode::Ports => ViewMode::Process,
                ViewMode::DockerEnv => ViewMode::Docker,
                _ => ViewMode::Ports,
            };
            state.selected = 0;
            let label = match state.view_mode {
                ViewMode::Process => "Process",
                ViewMode::Docker => "Docker",
                ViewMode::DockerEnv => "Docker Env",
                ViewMode::Ports => "Ports",
            };
            state.set_message(format!("View: {label}"));
        }
        KeyCode::Char('k') => {
            if state.view_mode == ViewMode::Process {
                kill_selected_process(state, system);
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
            if state.view_mode == ViewMode::Docker {
                open_selected_container_env(state);
            } else {
                state.set_message("Env only available in Docker view");
            }
        }
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.selected + 1 < list_len {
                state.selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.selected = state.selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            state.selected = (state.selected + 10).min(list_len.saturating_sub(1));
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

fn handle_docker_env_mode(key: KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => {
            state.view_mode = ViewMode::Docker;
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Up => {
            if state.docker_env_selected > 0 {
                state.docker_env_selected -= 1;
            }
        }
        KeyCode::Down => {
            if state.docker_env_selected + 1 < state.docker_env_vars.len() {
                state.docker_env_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.docker_env_selected = state.docker_env_selected.saturating_sub(10);
        }
        KeyCode::PageDown => {
            if !state.docker_env_vars.is_empty() {
                state.docker_env_selected =
                    (state.docker_env_selected + 10).min(state.docker_env_vars.len() - 1);
            }
        }
        _ => {}
    }
    false
}

fn kill_selected_process(state: &mut AppState, system: &mut System) {
    let Some(pid) = state.visible_pids.get(state.selected).cloned() else {
        state.set_message("No process selected");
        return;
    };

    match system.process(pid) {
        Some(process) => {
            let name = process.name().to_string();
            if process.kill() {
                state.set_message(format!("Killed PID {} ({})", pid, name));
            } else {
                state.set_message(format!("Failed to kill PID {} ({})", pid, name));
            }
        }
        None => {
            state.set_message(format!("Process PID {} not found", pid));
        }
    }
}

fn kill_selected_port_process(state: &mut AppState, system: &mut System) {
    let Some(pid) = state.visible_ports.get(state.selected).cloned() else {
        state.set_message("No port selected");
        return;
    };
    if pid == Pid::from_u32(0) {
        let container_id = state
            .visible_ports_container_ids
            .get(state.selected)
            .and_then(|id| id.clone());
        if let Some(id) = container_id {
            match docker::kill_container(&id) {
                Ok(()) => {
                    state.set_message(format!("Killed container {}", id));
                }
                Err(err) => {
                    state.set_message(format!("Failed to kill container: {err}"));
                }
            }
        } else {
            state.set_message("No process associated with this port");
        }
        return;
    }

    match system.process(pid) {
        Some(process) => {
            let name = process.name().to_string();
            if process.kill() {
                state.set_message(format!("Killed PID {} ({})", pid, name));
            } else {
                state.set_message(format!("Failed to kill PID {} ({})", pid, name));
            }
        }
        None => {
            state.set_message(format!("Process PID {} not found", pid));
        }
    }
}

fn open_selected_container(state: &mut AppState) {
    let Some(container_id) = state.visible_containers.get(state.selected) else {
        state.set_message("No container selected");
        return;
    };

    match docker::open_container_shell(container_id) {
        Ok(()) => {
            state.set_message(format!("Opening shell in {container_id}"));
        }
        Err(err) => {
            state.set_message(format!("Failed to open terminal: {err}"));
        }
    }
}

fn open_selected_container_logs(state: &mut AppState) {
    let Some(container_id) = state.visible_containers.get(state.selected) else {
        state.set_message("No container selected");
        return;
    };

    match docker::open_container_logs(container_id) {
        Ok(()) => {
            state.set_message(format!("Opening logs for {container_id}"));
        }
        Err(err) => {
            state.set_message(format!("Failed to open logs: {err}"));
        }
    }
}

fn open_selected_container_env(state: &mut AppState) {
    let Some(container_id) = state.visible_containers.get(state.selected).cloned() else {
        state.set_message("No container selected");
        return;
    };
    let name = state
        .visible_container_names
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| container_id.clone());
    let compose_name = state
        .visible_container_group_name
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let compose_path = state
        .visible_container_group_path
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let port_public = state
        .visible_container_ports_public
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let port_internal = state
        .visible_container_ports_internal
        .get(state.selected)
        .cloned()
        .unwrap_or_else(|| "-".to_string());

    state.input_mode = InputMode::Normal;
    state.view_mode = ViewMode::DockerEnv;
    state.docker_env_name = Some(name);
    state.docker_env_compose_name = compose_name;
    state.docker_env_compose_path = compose_path;
    state.docker_env_port_public = port_public;
    state.docker_env_port_internal = port_internal;
    state.docker_env_selected = 0;
    match docker::load_container_env(&container_id) {
        Ok(envs) => state.docker_env_vars = envs,
        Err(err) => {
            state.docker_env_vars = vec![format!("Failed to load env: {err}")];
        }
    }
}
