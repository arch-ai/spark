use std::collections::HashMap;
use std::time::{Duration, Instant};

use sysinfo::{Pid, Uid};

use crate::system::docker::DockerRow;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
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
    pub visible_container_ports_public: Vec<String>,
    pub visible_container_ports_internal: Vec<String>,
    pub visible_container_group_name: Vec<String>,
    pub visible_container_group_path: Vec<String>,
    pub docker_selected_row: usize,
    pub docker_rows: Vec<DockerRow>,
    pub hover_row: Option<usize>,
    pub sidebar_hover: Option<usize>,
    pub visible_ports: Vec<Pid>,
    pub visible_ports_container_ids: Vec<Option<String>>,
    pub visible_node_selectable: Vec<bool>,
    pub container_cache: HashMap<String, String>,
    pub container_last_refresh: Instant,
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
}

impl AppState {
    pub fn new() -> Self {
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
            sidebar_hover: None,
            visible_ports: Vec::new(),
            visible_ports_container_ids: Vec::new(),
            visible_node_selectable: Vec::new(),
            container_cache: HashMap::new(),
            container_last_refresh: Instant::now() - Duration::from_secs(60),
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
        }
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
