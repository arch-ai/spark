use std::path::Path;
use std::process::Command;

use sysinfo::Pid;

use super::PortInfo;

pub fn load_docker_port_bindings() -> Vec<PortInfo> {
    let output = Command::new("docker")
        .args([
            "ps",
            "--format",
            "{{.ID}}|{{.Names}}|{{.Image}}|{{.Ports}}|{{.Labels}}",
        ])
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
        let mut parts = line.splitn(5, '|');
        let id = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        let image = parts.next().unwrap_or("").trim();
        let ports_raw = parts.next().unwrap_or("").trim();
        let labels = parts.next().unwrap_or("").trim();
        if id.is_empty() || name.is_empty() || ports_raw.is_empty() {
            continue;
        }
        let group_name = compose_group_from_labels(labels);
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
                group_name: group_name.clone(),
                project_name: group_name.clone(),
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

fn compose_group_from_labels(labels: &str) -> Option<String> {
    let mut project = None;
    let mut working_dir = None;
    for part in labels.split(',') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let value = kv.next().unwrap_or("").trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        match key {
            "com.docker.compose.project.working_dir" => {
                working_dir = Some(value.to_string());
            }
            "com.docker.compose.project" => {
                project = Some(value.to_string());
            }
            _ => {}
        }
    }

    if let Some(dir) = working_dir {
        let path = Path::new(&dir);
        if let Some(name) = path.file_name() {
            return Some(name.to_string_lossy().into_owned());
        }
        if !dir.is_empty() {
            return Some(dir);
        }
    }

    project
}
