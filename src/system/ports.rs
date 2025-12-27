use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use sysinfo::{Pid, System};

pub struct PortInfo {
    pub proto: String,
    pub port: u16,
    pub pid: Pid,
    pub name: String,
    pub exe_path: String,
    pub container_id: Option<String>,
}

pub fn collect_ports(system: &System) -> Vec<PortInfo> {
    let inode_map = build_inode_pid_map();
    let mut rows = Vec::new();

    parse_socket_table("/proc/net/tcp", "tcp", Some("0A"), &inode_map, system, &mut rows);
    parse_socket_table("/proc/net/tcp6", "tcp6", Some("0A"), &inode_map, system, &mut rows);
    parse_socket_table("/proc/net/udp", "udp", None, &inode_map, system, &mut rows);
    parse_socket_table("/proc/net/udp6", "udp6", None, &inode_map, system, &mut rows);

    let mut seen = HashSet::new();
    for row in &rows {
        seen.insert((row.proto.clone(), row.port));
    }

    for docker_row in load_docker_port_bindings() {
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

fn parse_socket_table(
    path: &str,
    proto: &str,
    state_filter: Option<&str>,
    inode_map: &HashMap<u64, Pid>,
    system: &System,
    out: &mut Vec<PortInfo>,
) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };

    for line in contents.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        let local = parts[1];
        let state = parts[3];
        let inode_str = parts[9];

        if let Some(filter) = state_filter {
            if state != filter {
                continue;
            }
        }

        let port = parse_port(local);
        if port == 0 {
            continue;
        }
        let inode: u64 = inode_str.parse().unwrap_or(0);
        if inode == 0 {
            continue;
        }
        let Some(pid) = inode_map.get(&inode) else {
            continue;
        };
        let Some(process) = system.process(*pid) else {
            continue;
        };

        let name = process.name().to_string();
        let exe_path = process
            .exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "-".to_string());

        out.push(PortInfo {
            proto: proto.to_string(),
            port,
            pid: *pid,
            name,
            exe_path,
            container_id: None,
        });
    }
}

fn parse_port(local: &str) -> u16 {
    let mut parts = local.split(':');
    parts.next();
    let port_hex = parts.next().unwrap_or("");
    u16::from_str_radix(port_hex, 16).unwrap_or(0)
}

fn build_inode_pid_map() -> HashMap<u64, Pid> {
    let mut map = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return map;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let Ok(pid_u32) = name.parse::<u32>() else {
            continue;
        };
        let pid = Pid::from_u32(pid_u32);
        let fd_path = entry.path().join("fd");
        let Ok(fd_entries) = fs::read_dir(fd_path) else {
            continue;
        };
        for fd in fd_entries.flatten() {
            if let Ok(target) = fs::read_link(fd.path()) {
                if let Some(inode) = parse_socket_inode(&target) {
                    map.entry(inode).or_insert(pid);
                }
            }
        }
    }

    map
}

fn parse_socket_inode(path: &Path) -> Option<u64> {
    let link = path.to_string_lossy();
    if !link.starts_with("socket:[") || !link.ends_with(']') {
        return None;
    }
    let inner = link.trim_start_matches("socket:[").trim_end_matches(']');
    inner.parse::<u64>().ok()
}

fn load_docker_port_bindings() -> Vec<PortInfo> {
    let output = Command::new("docker")
        .args(["ps", "--format", "{{.ID}}|{{.Names}}|{{.Image}}|{{.Ports}}"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '|');
        let id = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        let image = parts.next().unwrap_or("").trim();
        let ports_raw = parts.next().unwrap_or("").trim();
        if id.is_empty() || name.is_empty() || ports_raw.is_empty() {
            continue;
        }
        for binding in parse_docker_port_bindings(ports_raw) {
            let label = format!("docker:{name}");
            let path = if binding.container_port > 0 {
                format!("image:{image} int:{}", binding.container_port)
            } else {
                format!("image:{image}")
            };
            rows.push(PortInfo {
                proto: binding.proto,
                port: binding.port,
                pid: Pid::from_u32(0),
                name: label,
                exe_path: path,
                container_id: Some(id.to_string()),
            });
        }
    }
    rows
}

struct DockerBinding {
    proto: String,
    port: u16,
    container_port: u16,
}

fn parse_docker_port_bindings(raw: &str) -> Vec<DockerBinding> {
    let mut bindings = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || !entry.contains("->") {
            continue;
        }
        let Some((left, right)) = entry.split_once("->") else {
            continue;
        };
        let host_port_str = extract_host_port(left.trim());
        let (container_port_str, proto) = split_port_proto(right.trim());
        let host_ports = parse_port_range(&host_port_str);
        let container_ports = parse_port_range(&container_port_str);
        let mapped = if !container_ports.is_empty() {
            container_ports
        } else {
            vec![0]
        };

        for (idx, host) in host_ports.iter().enumerate() {
            let container_port = mapped
                .get(idx)
                .cloned()
                .or_else(|| mapped.first().cloned())
                .unwrap_or(0);
            bindings.push(DockerBinding {
                proto: proto.clone(),
                port: *host,
                container_port,
            });
        }
    }
    bindings
}

fn extract_host_port(input: &str) -> String {
    let mut parts = input.rsplit(':');
    parts.next().unwrap_or("").trim().to_string()
}

fn split_port_proto(input: &str) -> (String, String) {
    if let Some((port, proto)) = input.split_once('/') {
        (port.trim().to_string(), proto.trim().to_string())
    } else {
        (input.trim().to_string(), "tcp".to_string())
    }
}

fn parse_port_range(input: &str) -> Vec<u16> {
    if input.is_empty() {
        return Vec::new();
    }
    if let Some((start, end)) = input.split_once('-') {
        let Ok(start) = start.trim().parse::<u16>() else {
            return Vec::new();
        };
        let Ok(end) = end.trim().parse::<u16>() else {
            return Vec::new();
        };
        if end < start {
            return Vec::new();
        }
        let count = end.saturating_sub(start) as usize + 1;
        if count > 1024 {
            return Vec::new();
        }
        return (start..=end).collect();
    }
    input.trim().parse::<u16>().map(|val| vec![val]).unwrap_or_default()
}
