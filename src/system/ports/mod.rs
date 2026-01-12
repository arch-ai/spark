mod docker;
mod proc;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use sysinfo::{Pid, System};

use crate::util::{contains_lower, Filterable};

#[derive(Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub proto: String,
    pub port: u16,
    pub internal_port: Option<u16>,
    pub pid: Pid,
    pub name: String,
    pub exe_path: String,
    pub container_id: Option<String>,
    pub group_name: Option<String>,
    pub project_name: Option<String>,
}

impl PortInfo {
    /// Returns the port binding display string (ext:int or just port)
    pub fn binding_display(&self) -> String {
        match self.internal_port {
            Some(int_port) if int_port > 0 && int_port != self.port => {
                format!("{}:{}", self.port, int_port)
            }
            _ => format!("{}", self.port),
        }
    }
}

pub enum PortRow {
    Group { name: String },
    Item { index: usize, prefix: String },
}

impl Filterable for PortInfo {
    fn matches_filter(&self, filter_lower: &str) -> bool {
        contains_lower(&self.proto, filter_lower)
            || self.port.to_string().contains(filter_lower)
            || self.pid.to_string().contains(filter_lower)
            || contains_lower(&self.name, filter_lower)
            || contains_lower(&self.exe_path, filter_lower)
            || self.container_id.as_deref().map_or(false, |c| contains_lower(c, filter_lower))
            || self.group_name.as_deref().map_or(false, |g| contains_lower(g, filter_lower))
            || self
                .project_name
                .as_deref()
                .map_or(false, |p| contains_lower(p, filter_lower))
    }
}

pub fn collect_ports(system: &System) -> Vec<PortInfo> {
    let inode_map = proc::build_inode_pid_map();
    let mut rows = proc::collect_proc_ports(system, &inode_map);

    let mut seen_proc = HashSet::new();
    let mut deduped = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if seen_proc.insert((row.proto.clone(), row.port, row.pid)) {
            deduped.push(row);
        }
    }
    rows = deduped;

    let mut seen_ports: HashSet<(String, u16)> = HashSet::new();
    for row in &rows {
        seen_ports.insert((row.proto.clone(), row.port));
    }

    // Deduplicate docker ports by (proto, port, container_id)
    let mut seen_docker: HashSet<(String, u16, String)> = HashSet::new();
    for docker_row in docker::load_docker_port_bindings() {
        // Skip if proc already has this port
        if seen_ports.contains(&(docker_row.proto.clone(), docker_row.port)) {
            continue;
        }
        // Skip duplicate docker entries for same container+port
        let container_id = docker_row.container_id.clone().unwrap_or_default();
        if !seen_docker.insert((docker_row.proto.clone(), docker_row.port, container_id)) {
            continue;
        }
        rows.push(docker_row);
    }

    rows.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then_with(|| a.proto.cmp(&b.proto))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    rows
}

pub fn group_ports(ports: &[PortInfo]) -> Vec<PortRow> {
    if ports.is_empty() {
        return Vec::new();
    }

    let mut labels = Vec::with_capacity(ports.len());
    let mut tokens = Vec::with_capacity(ports.len());
    let mut token_keys = Vec::with_capacity(ports.len());
    let mut token_counts: HashMap<String, usize> = HashMap::new();

    let mut groups: Vec<PortGroup> = Vec::new();
    let mut group_map: HashMap<String, usize> = HashMap::new();

    for port in ports {
        let label = group_label_for_port(port);
        let token = group_token_from_label(&label);
        let token_key = token.to_ascii_lowercase();
        if !token_key.is_empty() {
            *token_counts.entry(token_key.clone()).or_insert(0) += 1;
        }
        labels.push(label);
        tokens.push(token);
        token_keys.push(token_key);
    }

    for idx in 0..ports.len() {
        let use_token = token_counts
            .get(&token_keys[idx])
            .copied()
            .unwrap_or(0)
            > 1;
        let (group_key, group_label) = if use_token && !token_keys[idx].is_empty() {
            (
                format!("token::{}", token_keys[idx]),
                tokens[idx].clone(),
            )
        } else {
            (
                format!("label::{}", labels[idx].to_ascii_lowercase()),
                labels[idx].clone(),
            )
        };
        let group_index = match group_map.get(&group_key).copied() {
            Some(index) => index,
            None => {
                let index = groups.len();
                groups.push(PortGroup {
                    name: group_label,
                    items: Vec::new(),
                });
                group_map.insert(group_key.clone(), index);
                index
            }
        };
        groups[group_index].items.push(idx);
    }

    let mut rows = Vec::with_capacity(ports.len() + groups.len());
    for group in groups {
        rows.push(PortRow::Group {
            name: group.name,
        });
        let item_count = group.items.len();
        for (i, index) in group.items.into_iter().enumerate() {
            let prefix = if i + 1 == item_count {
                "  └─ ".to_string()
            } else {
                "  ├─ ".to_string()
            };
            rows.push(PortRow::Item { index, prefix });
        }
    }

    rows
}

struct PortGroup {
    name: String,
    items: Vec<usize>,
}

fn group_label_for_port(port: &PortInfo) -> String {
    if let Some(project_name) = port.project_name.as_ref() {
        let clean = project_name.trim();
        if !clean.is_empty() {
            return clean.to_string();
        }
    }
    if let Some(group_name) = port.group_name.as_ref() {
        let clean = group_name.trim();
        if !clean.is_empty() {
            return clean.to_string();
        }
    }
    display_group_label(&port.name)
}

fn group_token_from_label(label: &str) -> String {
    let trimmed = label.trim();
    let mut end = trimmed.len();
    for (idx, ch) in trimmed.char_indices() {
        if ch.is_whitespace() || matches!(ch, '|' | ':' | '-' | '_') {
            end = idx;
            break;
        }
    }
    let token = trimmed[..end].trim();
    if token.is_empty() {
        trimmed.to_string()
    } else {
        token.to_string()
    }
}

fn display_group_label(name: &str) -> String {
    let trimmed = name.trim();
    let after_colon = trimmed
        .rsplit_once(':')
        .map(|(_, tail)| tail.trim())
        .unwrap_or(trimmed);
    if after_colon.is_empty() {
        trimmed.to_string()
    } else {
        after_colon.to_string()
    }
}

/// Background worker for collecting ports data
pub struct PortsWorker {
    data: Arc<RwLock<Arc<Vec<PortInfo>>>>,
}

impl PortsWorker {
    /// Get a snapshot of the current ports data (thread-safe, no cloning)
    pub fn snapshot(&self) -> Arc<Vec<PortInfo>> {
        let guard = self.data.read().unwrap();
        guard.clone()
    }
}

/// Start a background worker that collects ports at the given interval
pub fn start_ports_worker(interval: Duration) -> PortsWorker {
    let data = Arc::new(RwLock::new(Arc::new(Vec::new())));
    let thread_data = Arc::clone(&data);

    thread::spawn(move || loop {
        let mut system = System::new();
        system.refresh_processes();
        let new_ports = collect_ports(&system);

        // Only update if data changed (avoid creating new Arc if unchanged)
        let mut guard = thread_data.write().unwrap_or_else(|err| err.into_inner());
        let current_ports: &Vec<PortInfo> = &guard;

        // Simple length check first (fast), then deep compare if needed
        if current_ports.len() != new_ports.len() || current_ports != &new_ports {
            *guard = Arc::new(new_ports);
        }

        drop(guard);
        thread::sleep(interval);
    });

    PortsWorker { data }
}
