use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;

use crate::app::{SortBy, SortOrder};
use crate::util::cmp_f32;

use super::{ContainerInfo, DockerRow};

#[derive(Clone)]
struct ComposeGroup {
    name: String,
    path: Option<String>,
}

pub fn load_docker_stats() -> Option<Vec<ContainerInfo>> {
    let meta = load_docker_container_meta().unwrap_or_default();
    let output = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.ID}}|{{.Name}}|{{.CPUPerc}}|{{.MemUsage}}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut containers = Vec::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }

        let mut parts = line.splitn(4, '|');
        let id = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        let cpu_raw = parts.next().unwrap_or("").trim();
        let mem_raw = parts.next().unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }

        let (meta_name, meta_image, meta_ports_public, meta_ports_internal, meta_status, meta_group) =
            resolve_meta(&meta, id);
        let resolved_name = if name.is_empty() { meta_name } else { name.to_string() };
        let resolved_image = meta_image.unwrap_or_else(|| "-".to_string());
        let resolved_ports_public = meta_ports_public.unwrap_or_else(|| "-".to_string());
        let resolved_ports_internal = meta_ports_internal.unwrap_or_else(|| "-".to_string());
        let resolved_status = meta_status.unwrap_or_else(|| "-".to_string());
        let group = meta_group.unwrap_or_else(|| ComposeGroup {
            name: "Other".to_string(),
            path: None,
        });

        let cpu = parse_cpu_percent(cpu_raw).unwrap_or(0.0);
        let mem_bytes = parse_mem_usage(mem_raw).unwrap_or(0);
        containers.push(ContainerInfo {
            id: id.to_string(),
            name: resolved_name,
            image: resolved_image,
            port_public: resolved_ports_public,
            port_internal: resolved_ports_internal,
            status: resolved_status,
            cpu,
            memory_bytes: mem_bytes,
            group_name: group.name,
            group_path: group.path,
        });
    }

    Some(containers)
}

pub fn group_containers(
    containers: Vec<ContainerInfo>,
    sort_by: SortBy,
    sort_order: SortOrder,
) -> (Vec<ContainerInfo>, Vec<DockerRow>) {
    struct GroupBucket {
        name: String,
        path: Option<String>,
        containers: Vec<ContainerInfo>,
    }

    let mut grouped: BTreeMap<String, GroupBucket> = BTreeMap::new();

    for container in containers {
        let key = container
            .group_path
            .clone()
            .unwrap_or_else(|| container.group_name.clone());
        let bucket = grouped.entry(key).or_insert_with(|| GroupBucket {
            name: container.group_name.clone(),
            path: container.group_path.clone(),
            containers: Vec::new(),
        });
        bucket.containers.push(container);
    }

    let other = grouped.remove("Other");

    let mut flat = Vec::new();
    let mut rows = Vec::new();
    let mut first_group = true;
    for (_key, mut bucket) in grouped {
        sort_containers(&mut bucket.containers, sort_by, sort_order);
        if !first_group {
            rows.push(DockerRow::Separator);
        }
        first_group = false;
        rows.push(DockerRow::Group {
            name: bucket.name.clone(),
            path: bucket.path.clone(),
            count: bucket.containers.len(),
        });
        let total = bucket.containers.len();
        for (idx, container) in bucket.containers.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            let prefix = if is_last { "  └─ " } else { "  ├─ " };
            let idx = flat.len();
            flat.push(container);
            rows.push(DockerRow::Item {
                index: idx,
                prefix: prefix.to_string(),
            });
        }
    }

    if let Some(mut bucket) = other {
        sort_containers(&mut bucket.containers, sort_by, sort_order);
        if !rows.is_empty() {
            rows.push(DockerRow::Separator);
        }
        rows.push(DockerRow::Group {
            name: bucket.name.clone(),
            path: bucket.path.clone(),
            count: bucket.containers.len(),
        });
        let total = bucket.containers.len();
        for (idx, container) in bucket.containers.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            let prefix = if is_last { "  └─ " } else { "  ├─ " };
            let idx = flat.len();
            flat.push(container);
            rows.push(DockerRow::Item {
                index: idx,
                prefix: prefix.to_string(),
            });
        }
    }

    (flat, rows)
}

pub fn apply_container_filter(containers: &mut Vec<ContainerInfo>, filter: &str) {
    if filter.is_empty() {
        return;
    }

    let filter_lower = filter.to_lowercase();
    containers.retain(|container| {
        container.id.to_lowercase().contains(&filter_lower)
            || container.name.to_lowercase().contains(&filter_lower)
            || container.image.to_lowercase().contains(&filter_lower)
            || container.port_public.to_lowercase().contains(&filter_lower)
            || container.port_internal.to_lowercase().contains(&filter_lower)
            || container.status.to_lowercase().contains(&filter_lower)
            || container.group_name.to_lowercase().contains(&filter_lower)
            || container
                .group_path
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&filter_lower)
    });
}

pub fn sort_containers(containers: &mut [ContainerInfo], sort_by: SortBy, sort_order: SortOrder) {
    match sort_by {
        SortBy::Cpu => containers.sort_by(|a, b| cmp_f32(a.cpu, b.cpu)),
        SortBy::Memory => containers.sort_by(|a, b| a.memory_bytes.cmp(&b.memory_bytes)),
        SortBy::Name => containers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }

    if sort_order == SortOrder::Desc {
        containers.reverse();
    }
}

fn load_docker_container_meta(
) -> Option<HashMap<String, (String, String, String, String, String, Option<ComposeGroup>)>> {
    let output = Command::new("docker")
        .args([
            "ps",
            "--no-trunc",
            "--format",
            "{{.ID}}|{{.Names}}|{{.Image}}|{{.Ports}}|{{.Status}}|{{.Labels}}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut meta = HashMap::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(6, '|');
        let id = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        let image = parts.next().unwrap_or("").trim();
        let ports_raw = parts.next().unwrap_or("").trim();
        let status = parts.next().unwrap_or("").trim();
        let labels = parts.next().unwrap_or("").trim();
        if id.is_empty() || name.is_empty() {
            continue;
        }
        let (ports_public, ports_internal) = parse_docker_ports(ports_raw);
        let group = compose_group_from_labels(labels);
        meta.insert(
            id.to_string(),
            (
                name.to_string(),
                image.to_string(),
                ports_public.clone(),
                ports_internal.clone(),
                status.to_string(),
                group.clone(),
            ),
        );
        if id.len() >= 12 {
            meta.insert(
                id[..12].to_string(),
                (
                    name.to_string(),
                    image.to_string(),
                    ports_public.clone(),
                    ports_internal.clone(),
                    status.to_string(),
                    group.clone(),
                ),
            );
        }
    }

    Some(meta)
}

fn resolve_meta(
    meta: &HashMap<String, (String, String, String, String, String, Option<ComposeGroup>)>,
    id: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<ComposeGroup>,
) {
    if let Some((name, image, ports_public, ports_internal, status, group)) = meta.get(id) {
        return (
            name.clone(),
            Some(image.clone()),
            Some(ports_public.clone()),
            Some(ports_internal.clone()),
            Some(status.clone()),
            group.clone(),
        );
    }
    if id.len() > 12 {
        if let Some((name, image, ports_public, ports_internal, status, group)) =
            meta.get(&id[..12])
        {
            return (
                name.clone(),
                Some(image.clone()),
                Some(ports_public.clone()),
                Some(ports_internal.clone()),
                Some(status.clone()),
                group.clone(),
            );
        }
    }
    ("-".to_string(), None, None, None, None, None)
}

fn compose_group_from_labels(labels: &str) -> Option<ComposeGroup> {
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
            return Some(ComposeGroup {
                name: name.to_string_lossy().into_owned(),
                path: Some(dir),
            });
        }
        if !dir.is_empty() {
            return Some(ComposeGroup {
                name: dir.clone(),
                path: Some(dir),
            });
        }
    }

    project.map(|name| ComposeGroup { name, path: None })
}

fn parse_cpu_percent(input: &str) -> Option<f32> {
    let trimmed = input.trim().trim_end_matches('%').replace(',', ".");
    trimmed.parse::<f32>().ok()
}

fn parse_mem_usage(input: &str) -> Option<u64> {
    let mut parts = input.splitn(2, '/');
    let used = parts.next()?.trim();
    parse_docker_size(used)
}

fn parse_docker_size(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut number = String::new();
    let mut unit = String::new();
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == ',' {
            number.push(ch);
        } else if !ch.is_whitespace() {
            unit.push(ch);
        }
    }

    if number.is_empty() {
        return None;
    }

    let normalized = number.replace(',', ".");
    let value: f64 = normalized.parse().ok()?;
    let unit_norm = unit.to_ascii_lowercase();
    let multiplier = match unit_norm.as_str() {
        "" | "b" => 1.0,
        "kb" => 1_000.0,
        "mb" => 1_000_000.0,
        "gb" => 1_000_000_000.0,
        "tb" => 1_000_000_000_000.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

fn parse_docker_ports(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ("-".to_string(), "-".to_string());
    }

    let mut public_ports = Vec::new();
    let mut internal_ports = Vec::new();
    let mut unbound_ports = Vec::new();

    for part in trimmed.split(',') {
        let entry = part.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some((left, right)) = entry.split_once("->") {
            let host_port = extract_host_port(left.trim());
            let internal_port = extract_container_port(right.trim());
            if !host_port.is_empty() {
                public_ports.push(host_port);
            }
            if !internal_port.is_empty() {
                internal_ports.push(internal_port);
            }
        } else {
            let port = extract_unbound_port(entry);
            if !port.is_empty() {
                unbound_ports.push(port);
            }
        }
    }

    if !public_ports.is_empty() {
        if !unbound_ports.is_empty() {
            internal_ports.extend(unbound_ports);
        }
        let pub_join = public_ports.join(",");
        let internal_join = internal_ports.join(",");
        let internal = if internal_join.is_empty() {
            "-".to_string()
        } else {
            internal_join
        };
        (pub_join, internal)
    } else if !unbound_ports.is_empty() {
        (unbound_ports.join(","), "-".to_string())
    } else {
        ("-".to_string(), "-".to_string())
    }
}

fn extract_host_port(input: &str) -> String {
    let mut parts = input.rsplit(':');
    parts.next().unwrap_or("").trim().to_string()
}

fn extract_container_port(input: &str) -> String {
    input
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn extract_unbound_port(input: &str) -> String {
    let trimmed = input.trim();
    if let Some((port, proto)) = trimmed.split_once('/') {
        let port = port.trim();
        let proto = proto.trim();
        if port.is_empty() {
            return String::new();
        }
        if proto.eq_ignore_ascii_case("tcp") || proto.is_empty() {
            port.to_string()
        } else {
            format!("{port}/{proto}")
        }
    } else {
        trimmed.to_string()
    }
}
