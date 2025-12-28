mod docker;
mod proc;

use std::collections::HashSet;

use sysinfo::{Pid, System};

use crate::util::{contains_lower, Filterable};

pub struct PortInfo {
    pub proto: String,
    pub port: u16,
    pub pid: Pid,
    pub name: String,
    pub exe_path: String,
    pub container_id: Option<String>,
}

impl Filterable for PortInfo {
    fn matches_filter(&self, filter_lower: &str) -> bool {
        contains_lower(&self.proto, filter_lower)
            || self.port.to_string().contains(filter_lower)
            || self.pid.to_string().contains(filter_lower)
            || contains_lower(&self.name, filter_lower)
            || contains_lower(&self.exe_path, filter_lower)
            || self.container_id.as_deref().map_or(false, |c| contains_lower(c, filter_lower))
    }
}

pub fn collect_ports(system: &System) -> Vec<PortInfo> {
    let inode_map = proc::build_inode_pid_map();
    let mut rows = proc::collect_proc_ports(system, &inode_map);

    let mut seen = HashSet::new();
    for row in &rows {
        seen.insert((row.proto.clone(), row.port));
    }

    for docker_row in docker::load_docker_port_bindings() {
        if seen.contains(&(docker_row.proto.clone(), docker_row.port)) {
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
