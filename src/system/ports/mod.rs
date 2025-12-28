mod docker;
mod proc;

use std::collections::{HashMap, HashSet};

use sysinfo::{Pid, System};

use crate::util::{contains_lower, Filterable};

pub struct PortInfo {
    pub proto: String,
    pub port: u16,
    pub pid: Pid,
    pub name: String,
    pub exe_path: String,
    pub container_id: Option<String>,
    pub group_name: Option<String>,
    pub project_name: Option<String>,
}

pub enum PortRow {
    Group { name: String, count: usize },
    Item { index: usize },
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

    let mut seen_ports = HashSet::new();
    for row in &rows {
        seen_ports.insert((row.proto.clone(), row.port));
    }

    for docker_row in docker::load_docker_port_bindings() {
        if seen_ports.contains(&(docker_row.proto.clone(), docker_row.port)) {
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
            count: group.items.len(),
        });
        for index in group.items {
            rows.push(PortRow::Item { index });
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
