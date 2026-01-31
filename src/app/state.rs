use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::text::Line;
use sysinfo::{Pid, Uid};

use crate::system::docker::{DockerListItem, DockerRow, DockerSystemDf};

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
    LogsNewWindow,
    Shell,
    Env,
    OpenLocation,
    Kill,
    DeleteImage,
    DeleteContainer,
    DeleteVolume,
    Inspect,
    ShowVolumes,
    ShowImages,
    ShowContainers,
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
            ContextMenuAction::LogsNewWindow => "] Logs - New Window",
            ContextMenuAction::Shell => "$ Shell",
            ContextMenuAction::Env => "# Env",
            ContextMenuAction::OpenLocation => "O Open Dir",
            ContextMenuAction::Kill => "x Kill",
            ContextMenuAction::DeleteImage => "x Delete Image",
            ContextMenuAction::DeleteContainer => "x Delete Container",
            ContextMenuAction::DeleteVolume => "x Delete Volume",
            ContextMenuAction::Inspect => "i Inspect",
            ContextMenuAction::ShowVolumes => "V Show Volumes",
            ContextMenuAction::ShowImages => "I Show Images",
            ContextMenuAction::ShowContainers => "C Show Containers",
            ContextMenuAction::PruneBuildCache => "P Prune Cache",
            ContextMenuAction::PruneDanglingImages => "P Prune Unused",
            ContextMenuAction::PruneVolumes => "P Prune Volumes",
        }
    }

    /// Returns true if this action is only available for single containers (not groups)
    pub fn is_container_only(&self) -> bool {
        matches!(
            self,
            ContextMenuAction::Logs | ContextMenuAction::LogsNewWindow | ContextMenuAction::Shell | ContextMenuAction::Env
        )
    }
}

#[derive(Clone, Debug)]
pub enum ContextMenuTarget {
    #[allow(dead_code)]
    Container { id: String, name: String, running: bool },
    DockerContainer { id: String, name: String },
    DockerImage { id: String, name: String },
    DockerVolume { name: String },
    Group { name: String, path: Option<String> },
    Process { pid: u32, name: String },
    Pm2 { pm_id: u32, name: String },
    DockerDf { kind: DockerDfKind },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DockerDfKind {
    Images,
    Containers,
    Volumes,
    BuildCache,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DockerListKind {
    Images,
    Containers,
    Volumes,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PruneConfirmChoice {
    Yes,
    No,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeleteConfirmChoice {
    Yes,
    No,
}

#[derive(Clone, Debug)]
pub struct PruneOutput {
    pub label: String,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct LogOutput {
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct DeleteConfirm {
    pub kind: DeleteKind,
    pub name: String,
    pub id: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeleteKind {
    Image,
    Container,
    Volume,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogOutputMode {
    Logs,
    Inspect,
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
    pub pm2_scroll: usize,
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
    /// Pending delete confirmation
    pub pending_delete: Option<DeleteConfirm>,
    /// Hovered choice in delete confirmation modal
    pub pending_delete_hover: Option<DeleteConfirmChoice>,
    /// Environment modal open
    pub env_modal_open: bool,
    /// Hover state for environment modal close button
    pub env_modal_hover: bool,
    /// Active log load label (shows progress spinner)
    pub log_in_progress: Option<String>,
    /// Log output modal contents
    pub log_output: Option<LogOutput>,
    /// Hover state for log output modal close button
    pub log_output_hover: bool,
    /// Hover state for log output modal select button
    pub log_select_hover: bool,
    /// Mouse selection mode for log output (disables mouse capture)
    pub log_select_mode: bool,
    /// Mode for log output modal rendering
    pub log_output_mode: LogOutputMode,
    /// Normalized log text for rendering
    pub log_text: String,
    /// Cached wrapped lines for log output
    pub log_lines: Vec<Line<'static>>,
    /// Width used for cached log wrapping
    pub log_wrap_width: u16,
    /// Cached line count for current wrap width
    pub log_line_count: usize,
    /// Current vertical scroll offset for the log output modal
    pub log_scroll: u16,
    /// Auto-follow newest log output
    pub log_follow: bool,
    /// Last time the log scroll position changed
    pub log_last_scroll: Instant,
    /// Log source for periodic refresh
    pub log_source: Option<LogSource>,
    /// Last time logs were refreshed
    pub log_last_refresh: Instant,
    /// Whether a log refresh is currently in flight
    pub log_refresh_in_progress: bool,
    /// Last time disk stats were refreshed
    pub last_disk_refresh: Instant,
    /// Cached terminal width for modal sizing
    pub term_width: u16,
    /// Cached terminal height for modal sizing
    pub term_height: u16,
    /// Hovered PM2 row in the Node view
    pub pm2_hover_row: Option<usize>,
    /// Docker list modal open
    pub docker_list_open: bool,
    /// Docker list modal kind (images/containers)
    pub docker_list_kind: Option<DockerListKind>,
    /// Docker list modal items
    pub docker_list_items: Vec<DockerListItem>,
    /// Selected index in docker list modal
    pub docker_list_selected: usize,
    /// Hover state for docker list modal close button
    pub docker_list_hover: bool,
}

#[derive(Clone, Debug)]
pub enum LogSource {
    Process { pid: u32 },
    Pm2 { pm_id: u32 },
    Docker { container_id: String },
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
            pm2_scroll: 0,
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
            docker_system_df: DockerSystemDf::default(),
            last_hover_render: Instant::now(),
            docker_df_hover: None,
            pending_prune: None,
            pending_prune_hover: None,
            prune_in_progress: None,
            prune_output: None,
            prune_output_hover: false,
            pending_delete: None,
            pending_delete_hover: None,
            env_modal_open: false,
            env_modal_hover: false,
            log_in_progress: None,
            log_output: None,
            log_output_hover: false,
            log_select_hover: false,
            log_select_mode: false,
            log_output_mode: LogOutputMode::Logs,
            log_text: String::new(),
            log_lines: Vec::new(),
            log_wrap_width: 0,
            log_line_count: 0,
            log_scroll: 0,
            log_follow: true,
            log_last_scroll: Instant::now() - Duration::from_secs(60),
            log_source: None,
            log_last_refresh: Instant::now() - Duration::from_secs(60),
            log_refresh_in_progress: false,
            last_disk_refresh: Instant::now() - Duration::from_secs(60),
            term_width: 0,
            term_height: 0,
            pm2_hover_row: None,
            docker_list_open: false,
            docker_list_kind: None,
            docker_list_items: Vec::new(),
            docker_list_selected: 0,
            docker_list_hover: false,
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
                any_completed = true;
                continue;
            }
            if let Some(title) = msg.container_id.strip_prefix("logs::") {
                if self.log_source.is_none() && self.log_output.is_none() && self.log_in_progress.is_none() {
                    self.log_refresh_in_progress = false;
                    continue;
                }
                self.log_in_progress = None;
                self.log_refresh_in_progress = false;
                let output = msg.output.clone().unwrap_or_default();
                self.set_log_output(title.to_string(), output);
                self.log_output_hover = false;
                if !msg.success && !msg.message.is_empty() {
                    self.set_message(format!("Failed to load logs: {}", msg.message));
                }
                any_completed = true;
                continue;
            }
            if let Some(title) = msg.container_id.strip_prefix("inspect::") {
                self.log_in_progress = None;
                self.log_refresh_in_progress = false;
                let output = msg
                    .output
                    .clone()
                    .unwrap_or_else(|| if msg.message.is_empty() { "No output.".to_string() } else { msg.message.clone() });
                self.set_log_output(title.to_string(), output);
                self.log_output_hover = false;
                self.log_select_hover = false;
                self.log_select_mode = false;
                self.log_output_mode = LogOutputMode::Inspect;
                self.log_follow = false;
                self.log_scroll = 0;
                self.log_source = None;
                if !msg.success && !msg.message.is_empty() {
                    self.set_message(format!("Failed to load inspect: {}", msg.message));
                }
                any_completed = true;
                continue;
            }
            if let Some(id) = msg.container_id.strip_prefix("image-delete::") {
                if msg.success {
                    if self.docker_list_open
                        && self.docker_list_kind == Some(DockerListKind::Images)
                    {
                        self.docker_list_items.retain(|item| item.id != id);
                        if self.docker_list_selected >= self.docker_list_items.len() {
                            self.docker_list_selected = self
                                .docker_list_items
                                .len()
                                .saturating_sub(1);
                        }
                    }
                }
                if !msg.message.is_empty() {
                    self.set_message(msg.message);
                }
                any_completed = true;
                continue;
            }
            if let Some(id) = msg.container_id.strip_prefix("container-delete::") {
                if msg.success {
                    if self.docker_list_open
                        && self.docker_list_kind == Some(DockerListKind::Containers)
                    {
                        self.docker_list_items.retain(|item| item.id != id);
                        if self.docker_list_selected >= self.docker_list_items.len() {
                            self.docker_list_selected = self
                                .docker_list_items
                                .len()
                                .saturating_sub(1);
                        }
                    }
                }
                if !msg.message.is_empty() {
                    self.set_message(msg.message);
                }
                any_completed = true;
                continue;
            }
            if let Some(name) = msg.container_id.strip_prefix("volume-delete::") {
                if msg.success {
                    if self.docker_list_open
                        && self.docker_list_kind == Some(DockerListKind::Volumes)
                    {
                        self.docker_list_items
                            .retain(|item| item.name != name);
                        if self.docker_list_selected >= self.docker_list_items.len() {
                            self.docker_list_selected = self
                                .docker_list_items
                                .len()
                                .saturating_sub(1);
                        }
                    }
                }
                if !msg.message.is_empty() {
                    self.set_message(msg.message);
                }
                any_completed = true;
                continue;
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

    pub fn log_max_scroll(&self, viewport_width: u16, viewport_height: u16) -> u16 {
        if self.log_output.is_none() || viewport_width == 0 || viewport_height == 0 {
            return 0;
        }
        let total_lines = if self.log_wrap_width == viewport_width && self.log_line_count > 0 {
            self.log_line_count
        } else {
            count_wrapped_lines(&self.log_text, viewport_width)
        };
        let max_scroll = total_lines.saturating_sub(viewport_height as usize);
        max_scroll.min(u16::MAX as usize) as u16
    }

    pub fn log_display_text(&self) -> &str {
        &self.log_text
    }

    pub fn ensure_log_lines(&mut self, width: u16) {
        if self.log_output.is_none() || width == 0 {
            return;
        }
        if self.log_wrap_width == width && !self.log_lines.is_empty() {
            return;
        }
        self.log_lines = wrap_text_lines(&self.log_text, width);
        self.log_line_count = self.log_lines.len();
        self.log_wrap_width = width;
    }

    pub fn clear_log_state(&mut self) {
        self.log_output = None;
        self.log_output_hover = false;
        self.log_select_hover = false;
        self.log_source = None;
        self.log_select_mode = false;
        self.log_output_mode = LogOutputMode::Logs;
        self.log_follow = true;
        self.log_scroll = 0;
        self.log_refresh_in_progress = false;
        self.log_last_scroll = Instant::now();
        self.log_text.clear();
        self.log_lines.clear();
        self.log_wrap_width = 0;
        self.log_line_count = 0;
    }

    fn set_log_output(&mut self, title: String, output: String) {
        self.log_output = Some(LogOutput { title });
        let raw = if output.trim().is_empty() {
            "No output.".to_string()
        } else {
            output
        };
        self.log_text = normalize_log_text(&raw);
        self.log_lines.clear();
        self.log_wrap_width = 0;
        self.log_line_count = 0;
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

fn normalize_log_text(text: &str) -> String {
    let stripped = strip_ansi(text);
    let mut out = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            '\r' => {}
            '\t' => out.push_str("    "),
            '\x08' => {}
            _ => out.push(ch),
        }
    }
    out
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut iter = text.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match iter.peek().copied() {
            Some('[') => {
                iter.next();
                while let Some(c) = iter.next() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                iter.next();
                loop {
                    match iter.next() {
                        Some('\x07') => break,
                        Some('\x1b') => {
                            if let Some('\\') = iter.peek().copied() {
                                iter.next();
                                break;
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn count_wrapped_lines(text: &str, width: u16) -> usize {
    let width = width as usize;
    if width == 0 {
        return 0;
    }
    let mut total = 0usize;
    for line in text.split('\n') {
        if line.is_empty() {
            total = total.saturating_add(1);
            continue;
        }
        let mut count = 0usize;
        let mut line_len = 0usize;
        for _ch in line.chars() {
            line_len += 1;
            if line_len >= width {
                count += 1;
                line_len = 0;
            }
        }
        if line_len > 0 {
            count += 1;
        }
        total = total.saturating_add(count.max(1));
    }
    if text.ends_with('\n') {
        total = total.saturating_add(1);
    }
    total
}

fn wrap_text_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    if width == 0 {
        return vec![Line::from("")];
    }
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        let mut buf = String::new();
        let mut count = 0usize;
        for ch in line.chars() {
            buf.push(ch);
            count += 1;
            if count >= width {
                out.push(Line::from(std::mem::take(&mut buf)));
                count = 0;
            }
        }
        if !buf.is_empty() {
            out.push(Line::from(buf));
        }
    }
    out
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
