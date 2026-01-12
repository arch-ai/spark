use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use sysinfo::{Pid, Uid};

use crate::system::docker::{DockerRow, DockerSystemDf};

/// Message sent when a container operation completes
#[derive(Debug)]
pub struct OperationComplete {
    pub container_id: String,
    pub success: bool,
    pub message: String,
    pub output: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Start,
    Stop,
    Restart,
    Logs,
    Shell,
    Env,
    Kill,
    PruneBuildCache,
    PruneDanglingImages,
    PruneVolumes,
}

impl ContextMenuAction {
    pub fn label(&self, is_group: bool) -> &'static str {
        match self {
            ContextMenuAction::Start => if is_group { "> Start All" } else { "> Start" },
            ContextMenuAction::Stop => if is_group { "x Stop All" } else { "x Stop" },
            ContextMenuAction::Restart => if is_group { "~ Restart All" } else { "~ Restart" },
            ContextMenuAction::Logs => "] Logs",
            ContextMenuAction::Shell => "$ Shell",
            ContextMenuAction::Env => "# Env",
            ContextMenuAction::Kill => "x Kill",
            ContextMenuAction::PruneBuildCache => "P Prune Cache",
            ContextMenuAction::PruneDanglingImages => "P Prune Images",
            ContextMenuAction::PruneVolumes => "P Prune Volumes",
        }
    }

    /// Returns true if this action is only available for single containers (not groups)
    pub fn is_container_only(&self) -> bool {
        matches!(self, ContextMenuAction::Logs | ContextMenuAction::Shell | ContextMenuAction::Env)
    }
}

#[derive(Clone, Debug)]
pub enum ContextMenuTarget {
    #[allow(dead_code)]
    Container { id: String, name: String, running: bool },
    Group { name: String, path: Option<String> },
    Process { pid: u32, name: String },
    DockerDf,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PruneConfirmChoice {
    Yes,
    No,
}

#[derive(Clone, Debug)]
pub struct PruneOutput {
    pub label: String,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub x: u16,
    pub y: u16,
    pub items: Vec<ContextMenuAction>,
    pub hover: Option<usize>,
    pub target: ContextMenuTarget,
    pub is_group: bool,
    pub header: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Process,
    Docker,
    DockerEnv,
    Ports,
    Node,
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
    pub node_filter: String,
    pub sort_by: SortBy,
    pub sort_order: SortOrder,
    pub zoom: bool,
    pub view_mode: ViewMode,
    pub focus: Focus,
    pub sidebar_index: usize,
    pub selected: usize,
    pub message: Option<String>,
    pub message_until: Option<Instant>,
    pub visible_pids: Vec<Pid>,
    pub visible_containers: Vec<String>,
    pub visible_container_names: Vec<String>,
    /// Uses Cow to avoid allocation when port is static "-"
    pub visible_container_ports_public: Vec<Cow<'static, str>>,
    /// Uses Cow to avoid allocation when port is static "-"
    pub visible_container_ports_internal: Vec<Cow<'static, str>>,
    /// Uses Cow to avoid allocation when group is static "Other"
    pub visible_container_group_name: Vec<Cow<'static, str>>,
    pub visible_container_group_path: Vec<String>,
    pub docker_selected_row: usize,
    pub docker_rows: Vec<DockerRow>,
    pub hover_row: Option<usize>,
    // Scroll offsets - tracked to avoid re-centering on click
    pub process_scroll: usize,
    pub docker_scroll: usize,
    pub ports_scroll: usize,
    pub node_scroll: usize,
    pub sidebar_hover: Option<usize>,
    pub context_menu: Option<ContextMenu>,
    pub visible_ports: Vec<Pid>,
    pub visible_ports_container_ids: Vec<Option<String>>,
    pub visible_node_selectable: Vec<bool>,
    pub user_cache: HashMap<Uid, String>,
    pub user_last_refresh: Instant,
    pub docker_filtered_out: usize,
    pub docker_total: usize,
    pub env_vars: Vec<String>,
    pub env_title: String,
    pub env_info_left1: String,
    pub env_info_right1: String,
    pub env_info_left2: String,
    pub env_info_right2: String,
    pub env_selected: usize,
    pub env_return_view: ViewMode,
    pub cpu_usage: f32,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub disk_total: u64,
    pub disk_available: u64,
    /// Maps container ID -> expected running state (true = should be running, false = should be stopped)
    pub pending_operations: HashMap<String, bool>,
    pub operation_tx: Sender<OperationComplete>,
    pub operation_rx: Receiver<OperationComplete>,
    /// Frame counter for animated spinner
    pub spinner_frame: usize,
    /// Last time spinner was ticked
    pub spinner_last_tick: Instant,
    /// Cached PM2 availability status
    pub pm2_available: bool,
    /// Last time PM2 availability was checked
    pub pm2_last_check: Instant,
    /// Docker system disk usage from `docker system df`
    pub docker_system_df: DockerSystemDf,
    /// Last time hover was rendered (for throttling)
    pub last_hover_render: Instant,
    /// Pending hover row that needs rendering
    /// Hover state for docker df stats rows (0-3 for Images, Containers, Volumes, Build Cache)
    pub docker_df_hover: Option<usize>,
    /// Pending prune confirmation action
    pub pending_prune: Option<ContextMenuAction>,
    /// Hovered choice in prune confirmation modal
    pub pending_prune_hover: Option<PruneConfirmChoice>,
    /// Active prune job label (shows progress spinner)
    pub prune_in_progress: Option<String>,
    /// Prune output modal contents
    pub prune_output: Option<PruneOutput>,
    /// Hover state for prune output modal close button
    pub prune_output_hover: bool,
}

impl AppState {
    pub fn new() -> Self {
        let (operation_tx, operation_rx) = mpsc::channel();
        Self {
            input_mode: InputMode::Normal,
            process_filter: String::new(),
            docker_filter: String::new(),
            ports_filter: String::new(),
            node_filter: String::new(),
            sort_by: SortBy::Memory,
            sort_order: SortOrder::Desc,
            zoom: false,
            view_mode: ViewMode::Process,
            focus: Focus::Main,
            sidebar_index: 0,
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
            docker_selected_row: 0,
            docker_rows: Vec::new(),
            hover_row: None,
            process_scroll: 0,
            docker_scroll: 0,
            ports_scroll: 0,
            node_scroll: 0,
            sidebar_hover: None,
            context_menu: None,
            visible_ports: Vec::new(),
            visible_ports_container_ids: Vec::new(),
            visible_node_selectable: Vec::new(),
            user_cache: HashMap::new(),
            user_last_refresh: Instant::now() - Duration::from_secs(60),
            docker_filtered_out: 0,
            docker_total: 0,
            env_vars: Vec::new(),
            env_title: "ENV".to_string(),
            env_info_left1: "-".to_string(),
            env_info_right1: "-".to_string(),
            env_info_left2: "-".to_string(),
            env_info_right2: "-".to_string(),
            env_selected: 0,
            env_return_view: ViewMode::Process,
            cpu_usage: 0.0,
            mem_total: 0,
            mem_available: 0,
            swap_total: 0,
            swap_used: 0,
            disk_total: 0,
            disk_available: 0,
            pending_operations: HashMap::new(),
            operation_tx,
            operation_rx,
            spinner_frame: 0,
            spinner_last_tick: Instant::now(),
            pm2_available: false,
            // Set to past time so first check happens immediately
            pm2_last_check: Instant::now() - Duration::from_secs(60),
            docker_system_df: DockerSystemDf::default(),
            last_hover_render: Instant::now(),
            docker_df_hover: None,
            pending_prune: None,
            pending_prune_hover: None,
            prune_in_progress: None,
            prune_output: None,
            prune_output_hover: false,
        }
    }

    /// Advance spinner animation frame, returns true if spinner actually changed
    pub fn tick_spinner(&mut self) -> bool {
        if !self.pending_operations.is_empty() || self.prune_in_progress.is_some() {
            // Rate limit spinner updates to ~8 fps (125ms interval)
            if self.spinner_last_tick.elapsed() >= Duration::from_millis(125) {
                self.spinner_frame = self.spinner_frame.wrapping_add(1);
                self.spinner_last_tick = Instant::now();
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get current spinner character
    pub fn spinner_char(&self) -> char {
        const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        SPINNER[self.spinner_frame % SPINNER.len()]
    }

    pub fn check_completed_operations(&mut self) -> bool {
        let mut any_completed = false;
        while let Ok(msg) = self.operation_rx.try_recv() {
            if msg.container_id.starts_with("prune-") {
                self.prune_in_progress = None;
                if let Some(output) = msg.output.clone() {
                    let label = msg
                        .container_id
                        .trim_start_matches("prune-")
                        .replace('-', " ");
                    self.prune_output = Some(PruneOutput { label, output });
                    self.prune_output_hover = false;
                }
            }
            // Only remove from pending on failure - success keeps it pending until state matches
            if !msg.success {
                self.pending_operations.remove(&msg.container_id);
            }
            if !msg.message.is_empty() {
                self.set_message(msg.message);
            }
            any_completed = true;
        }
        any_completed
    }

    /// Check container states and remove from pending when state matches expected
    pub fn update_pending_with_containers(&mut self, containers: &[crate::system::docker::ContainerInfo]) -> bool {
        if self.pending_operations.is_empty() {
            return false;
        }

        let mut to_remove = Vec::new();
        for (container_id, expected_running) in &self.pending_operations {
            // Find this container in the list
            if let Some(container) = containers.iter().find(|c| &c.id == container_id) {
                // If actual state matches expected state, operation is complete
                if container.running == *expected_running {
                    to_remove.push(container_id.clone());
                }
            }
        }

        let any_removed = !to_remove.is_empty();
        for id in to_remove {
            self.pending_operations.remove(&id);
        }
        any_removed
    }

    #[allow(dead_code)]
    pub fn is_container_pending(&self, container_id: &str) -> bool {
        self.pending_operations.contains_key(container_id)
    }

    pub(crate) fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.message_until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub(crate) fn clear_expired_message(&mut self) -> bool {
        if let Some(deadline) = self.message_until {
            if Instant::now() >= deadline {
                self.message = None;
                self.message_until = None;
                return true;
            }
        }
        false
    }

    pub(crate) fn toggle_sort(&mut self, sort_by: SortBy) {
        if self.sort_by == sort_by {
            self.sort_order = self.sort_order.toggle();
        } else {
            self.sort_by = sort_by;
            self.sort_order = SortOrder::Desc;
        }
    }

    pub(crate) fn set_view(&mut self, view: ViewMode) {
        self.view_mode = view;
        self.selected = 0;
        self.sidebar_index = sidebar_index_for_view(view);
    }

    /// Adjust scroll to keep selection visible without centering
    pub fn adjust_scroll(&mut self, visible_height: usize, total: usize) {
        let (scroll, selected) = match self.view_mode {
            ViewMode::Process => (&mut self.process_scroll, self.selected),
            ViewMode::Docker => (&mut self.docker_scroll, self.docker_selected_row),
            ViewMode::Ports => (&mut self.ports_scroll, self.selected),
            ViewMode::Node => (&mut self.node_scroll, self.selected),
            ViewMode::DockerEnv => return,
        };

        // Clamp scroll to valid range
        let max_scroll = total.saturating_sub(visible_height);
        if *scroll > max_scroll {
            *scroll = max_scroll;
        }

        // Scroll up if selection is above visible area
        if selected < *scroll {
            *scroll = selected;
        }
        // Scroll down if selection is below visible area
        else if selected >= *scroll + visible_height {
            *scroll = selected.saturating_sub(visible_height) + 1;
        }
    }

    /// Get current scroll offset for the active view
    #[allow(dead_code)]
    pub fn current_scroll(&self) -> usize {
        match self.view_mode {
            ViewMode::Process => self.process_scroll,
            ViewMode::Docker => self.docker_scroll,
            ViewMode::Ports => self.ports_scroll,
            ViewMode::Node => self.node_scroll,
            ViewMode::DockerEnv => 0,
        }
    }

    pub(crate) fn active_filter(&self) -> &str {
        match self.view_mode {
            ViewMode::Process => &self.process_filter,
            ViewMode::Docker | ViewMode::DockerEnv => &self.docker_filter,
            ViewMode::Ports => &self.ports_filter,
            ViewMode::Node => &self.node_filter,
        }
    }

    pub(crate) fn active_filter_mut(&mut self) -> &mut String {
        match self.view_mode {
            ViewMode::Process => &mut self.process_filter,
            ViewMode::Docker | ViewMode::DockerEnv => &mut self.docker_filter,
            ViewMode::Ports => &mut self.ports_filter,
            ViewMode::Node => &mut self.node_filter,
        }
    }

    pub(crate) fn is_ports_group_row(&self, index: usize) -> bool {
        let Some(pid) = self.visible_ports.get(index) else {
            return false;
        };
        let has_container = self
            .visible_ports_container_ids
            .get(index)
            .and_then(|id| id.as_ref())
            .is_some();
        pid.as_u32() == 0 && !has_container
    }

    pub(crate) fn is_node_selectable_row(&self, index: usize) -> bool {
        self.visible_node_selectable
            .get(index)
            .copied()
            .unwrap_or(false)
    }

    pub(crate) fn is_docker_selectable_row(&self, index: usize) -> bool {
        matches!(
            self.docker_rows.get(index),
            Some(DockerRow::Group { .. }) | Some(DockerRow::Item { .. })
        )
    }
}

pub(crate) fn sidebar_index_for_view(view: ViewMode) -> usize {
    match view {
        ViewMode::Process => 0,
        ViewMode::Ports => 1,
        ViewMode::Docker | ViewMode::DockerEnv => 2,
        ViewMode::Node => 3,
    }
}

pub(crate) fn view_for_sidebar_index(index: usize) -> ViewMode {
    match index {
        1 => ViewMode::Ports,
        2 => ViewMode::Docker,
        3 => ViewMode::Node,
        _ => ViewMode::Process,
    }
}
