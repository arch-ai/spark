use std::process::Command;

use sysinfo::Pid;

use super::PortInfo;

pub fn load_docker_port_bindings() -> Vec<PortInfo> {
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
