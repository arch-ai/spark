mod docker;
mod proc;

use crate::system::worker::{self, Worker};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
    Item { index: usize },
}

impl Filterable for PortInfo {
    fn matches_filter(&self, filter_lower: &str) -> bool {
        contains_lower(&self.proto, filter_lower)
            || self.port.to_string().contains(filter_lower)
            || self
                .internal_port
                .is_some_and(|port| port.to_string().contains(filter_lower))
            || self.pid.to_string().contains(filter_lower)
            || contains_lower(&self.name, filter_lower)
            || contains_lower(&self.exe_path, filter_lower)
            || self
                .container_id
                .as_deref()
                .map_or(false, |c| contains_lower(c, filter_lower))
            || self
                .group_name
                .as_deref()
                .map_or(false, |g| contains_lower(g, filter_lower))
            || self
                .project_name
                .as_deref()
                .map_or(false, |p| contains_lower(p, filter_lower))
    }
}

#[derive(Default)]
pub struct PortsSnapshot {
    pub ports: Arc<Vec<PortInfo>>,
    pub docker_error: Option<String>,
}

pub fn collect_ports(
    system: &System,
    last_docker: &mut Vec<PortInfo>,
) -> std::io::Result<PortsSnapshot> {
    let inode_map = proc::build_inode_pid_map();
    let host = proc::collect_proc_ports(system, &inode_map)?;
    Ok(ports_snapshot(
        host,
        last_docker,
        docker::load_docker_port_bindings(),
    ))
}

/// A one-shot native owner lookup bypasses the periodic worker's inode cache.
pub(crate) fn current_native_owner(protocol: &str, port: u16) -> std::io::Result<(u32, u64)> {
    let map = proc::build_inode_pid_map_uncached();
    let listeners = proc::collect_proc_ports(&System::new(), &map)?;
    let owners: std::collections::BTreeSet<_> = listeners
        .iter()
        .filter(|listener| {
            listener.proto == protocol && listener.port == port && listener.pid.as_u32() != 0
        })
        .map(|listener| listener.pid.as_u32())
        .collect();
    if owners.len() != 1 {
        return Err(std::io::Error::other(if owners.is_empty() {
            "Listener exited or its owner is inaccessible; refresh Ports"
        } else {
            "Several processes own this listener; inspect their rows before navigating"
        }));
    }
    let pid = *owners.first().unwrap();
    Ok((pid, crate::system::process::process_identity(pid)?))
}

fn ports_snapshot(
    host: Vec<PortInfo>,
    last_docker: &mut Vec<PortInfo>,
    docker_result: std::io::Result<Vec<PortInfo>>,
) -> PortsSnapshot {
    let docker_error = match docker_result {
        Ok(rows) => {
            *last_docker = rows;
            None
        }
        Err(err) => Some(format!(
            "Container ports {}: {err}",
            if last_docker.is_empty() {
                "unavailable"
            } else {
                "cached"
            }
        )),
    };
    PortsSnapshot {
        ports: Arc::new(merge_ports(host, last_docker.clone())),
        docker_error,
    }
}

fn merge_ports(mut rows: Vec<PortInfo>, docker_rows: Vec<PortInfo>) -> Vec<PortInfo> {
    let mut seen_proc = HashSet::new();
    let mut deduped = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if seen_proc.insert((row.proto.clone(), row.port, row.pid)) {
            deduped.push(row);
        }
    }
    rows = deduped;

    // Deduplicate docker ports by (proto, port, container_id)
    let mut seen_docker: HashSet<(String, u16, String)> = HashSet::new();
    for docker_row in docker_rows {
        // Replace proxy listeners with their container identity, without hiding
        // a real host process bound to the same port on another address.
        rows.retain(|row| {
            !(row.name == "docker-proxy"
                && row.port == docker_row.port
                && row.proto.trim_end_matches('6') == docker_row.proto.trim_end_matches('6'))
        });
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
        let use_token = token_counts.get(&token_keys[idx]).copied().unwrap_or(0) > 1;
        let (group_key, group_label) = if use_token && !token_keys[idx].is_empty() {
            (format!("token::{}", token_keys[idx]), tokens[idx].clone())
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
        rows.push(PortRow::Group { name: group.name });
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

/// Uses the same wakeable lifecycle as the other resource views.
pub struct PortsWorker(Worker<PortsSnapshot>);
impl PortsWorker {
    pub fn snapshot(&self) -> Arc<Vec<PortInfo>> {
        Arc::clone(&self.0.snapshot().data.ports)
    }
    pub fn error(&self) -> Option<String> {
        {
            let snapshot = self.0.snapshot();
            snapshot
                .error
                .clone()
                .or_else(|| snapshot.data.docker_error.clone())
        }
    }
    pub fn is_loaded(&self) -> bool {
        self.0.snapshot().updated_at.is_some()
    }
    pub fn set_paused(&self, paused: bool) {
        self.0.set_paused(paused);
    }
    pub fn refresh(&self) {
        self.0.refresh();
    }
}
pub fn start_ports_worker(interval: Duration) -> PortsWorker {
    let mut system = System::new();
    let mut last_docker = Vec::new();
    PortsWorker(worker::start_worker(interval, move || {
        system.refresh_processes_specifics(
            sysinfo::ProcessRefreshKind::new()
                .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
        );
        collect_ports(&system, &mut last_docker)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_navigation_resolves_a_current_socket_and_rejects_a_closed_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (pid, identity) = current_native_owner("tcp", port).unwrap();
        assert_eq!(pid, std::process::id());
        assert_eq!(
            identity,
            crate::system::process::process_identity(pid).unwrap()
        );
        drop(listener);
        assert!(current_native_owner("tcp", port).is_err());
    }
    fn binding(name: &str, proto: &str, pid: u32, id: Option<&str>) -> PortInfo {
        PortInfo {
            proto: proto.into(),
            port: 8080,
            internal_port: None,
            pid: Pid::from_u32(pid),
            name: name.into(),
            exe_path: "-".into(),
            container_id: id.map(str::to_owned),
            group_name: None,
            project_name: None,
        }
    }
    #[test]
    fn container_bindings_replace_only_matching_proxies_and_keep_protocols_and_host_owners() {
        let host = vec![
            binding("docker-proxy", "tcp6", 1, None),
            binding("docker-proxy", "udp", 2, None),
            binding("server", "tcp", 3, None),
        ];
        let mut container = binding("docker:api", "tcp", 0, Some("api-id"));
        container.internal_port = Some(80);
        let rows = merge_ports(host, vec![container.clone(), container]);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|row| row.name == "server"));
        assert!(rows.iter().any(|row| row.proto == "udp"));
        assert_eq!(
            rows.iter()
                .find(|row| row.container_id.is_some())
                .unwrap()
                .binding_display(),
            "8080:80"
        );
    }

    #[test]
    fn docker_failure_keeps_fresh_host_ports_and_cached_bindings_until_recovery() {
        let container = binding("docker:api", "tcp", 0, Some("api-id"));
        let mut cached = Vec::new();
        let good = ports_snapshot(Vec::new(), &mut cached, Ok(vec![container]));
        assert_eq!(good.ports.len(), 1);
        let failed = ports_snapshot(
            vec![binding("new-host", "udp", 9, None)],
            &mut cached,
            Err(std::io::Error::other("offline")),
        );
        assert_eq!(failed.ports.len(), 2);
        assert!(failed.ports.iter().any(|row| row.name == "new-host"));
        assert_eq!(
            failed.docker_error.as_deref(),
            Some("Container ports cached: offline")
        );
        let recovered = ports_snapshot(Vec::new(), &mut cached, Ok(Vec::new()));
        assert!(recovered.ports.is_empty());
        assert!(recovered.docker_error.is_none());
        assert!(cached.is_empty());
    }
}
