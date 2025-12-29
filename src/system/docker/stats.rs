use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::app::{SortBy, SortOrder};
use crate::util::cmp_f32;

use super::{ContainerInfo, DockerRow};

/// Static string constants to avoid repeated allocations
const DASH: &str = "-";
const OTHER: &str = "Other";

/// Load docker stats using a single combined command.
/// This reduces process spawning from 2 calls to 1 per refresh cycle.
/// Includes stopped containers with -a flag.
pub fn load_docker_stats() -> Option<Vec<ContainerInfo>> {
    // Combined format: stats data + metadata in single command
    // Format: ID|Name|CPU|MemUsage|Image|Ports|Status|Labels
    // Use -a to include stopped containers
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--no-trunc",
            "--format",
            "{{.ID}}|{{.Names}}|{{.Image}}|{{.Ports}}|{{.Status}}|{{.Labels}}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Parse container metadata from docker ps
    let stdout = String::from_utf8_lossy(&output.stdout);
    let container_ids: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                return None;
            }
            line.split('|').next().map(|s| s.trim())
        })
        .filter(|id| !id.is_empty())
        .collect();

    if container_ids.is_empty() {
        return Some(Vec::new());
    }

    // Get stats for all containers in a single call
    let stats_output = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.ID}}|{{.Name}}|{{.CPUPerc}}|{{.MemUsage}}",
        ])
        .output()
        .ok()?;

    if !stats_output.status.success() {
        return None;
    }

    // Build stats map (ID -> (cpu, mem))
    let stats_stdout = String::from_utf8_lossy(&stats_output.stdout);
    let mut stats_map: std::collections::HashMap<&str, (f32, u64)> =
        std::collections::HashMap::with_capacity(container_ids.len());

    for raw_line in stats_stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '|');
        let id = parts.next().unwrap_or("").trim();
        let _name = parts.next();
        let cpu_raw = parts.next().unwrap_or("").trim();
        let mem_raw = parts.next().unwrap_or("").trim();
        if !id.is_empty() {
            let cpu = parse_cpu_percent(cpu_raw).unwrap_or(0.0);
            let mem = parse_mem_usage(mem_raw).unwrap_or(0);
            stats_map.insert(id, (cpu, mem));
        }
    }

    // Parse metadata and combine with stats
    let mut containers = Vec::with_capacity(container_ids.len());
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

        // Lookup stats by full ID or short ID
        let (cpu, memory_bytes) = stats_map
            .get(id)
            .or_else(|| {
                if id.len() >= 12 {
                    stats_map.get(&id[..12])
                } else {
                    None
                }
            })
            .copied()
            .unwrap_or((0.0, 0));

        // Determine if container is running from status
        // Status starts with "Up" for running containers
        let running = status.starts_with("Up");
        let activity_secs = parse_activity_time(status);

        containers.push(ContainerInfo {
            id: id.to_string(),
            name: name.to_string(),
            image: if image.is_empty() {
                Cow::Borrowed(DASH)
            } else {
                Cow::Owned(image.to_string())
            },
            port_public: ports_public,
            port_internal: ports_internal,
            status: if status.is_empty() {
                Cow::Borrowed(DASH)
            } else {
                Cow::Owned(status.to_string())
            },
            cpu,
            memory_bytes,
            group_name: group
                .as_ref()
                .map(|g| Cow::Owned(g.name.clone()))
                .unwrap_or(Cow::Borrowed(OTHER)),
            group_path: group.and_then(|g| g.path),
            running,
            activity_secs,
        });
    }

    Some(containers)
}

#[derive(Clone)]
struct ComposeGroup {
    name: String,
    path: Option<String>,
}

pub fn group_containers(
    containers: Vec<ContainerInfo>,
    _sort_by: SortBy,
    _sort_order: SortOrder,
) -> (Vec<ContainerInfo>, Vec<DockerRow>) {
    struct GroupBucket {
        name: Cow<'static, str>,
        path: Option<String>,
        containers: Vec<ContainerInfo>,
        min_activity: u64, // Most recent activity in group
    }

    let mut grouped: BTreeMap<String, GroupBucket> = BTreeMap::new();

    for container in containers {
        let key = container
            .group_path
            .clone()
            .unwrap_or_else(|| container.group_name.to_string());
        let bucket = grouped.entry(key).or_insert_with(|| GroupBucket {
            name: container.group_name.clone(),
            path: container.group_path.clone(),
            containers: Vec::new(),
            min_activity: u64::MAX,
        });
        // Track the most recent activity in the group
        bucket.min_activity = bucket.min_activity.min(container.activity_secs);
        bucket.containers.push(container);
    }

    let other = grouped.remove("Other");

    // Convert to vec and sort by activity (most recent first)
    let mut buckets: Vec<_> = grouped.into_values().collect();
    buckets.sort_by_key(|b| b.min_activity);

    let mut flat = Vec::new();
    let mut rows = Vec::new();
    let mut first_group = true;

    for mut bucket in buckets {
        // Sort containers within group by activity (most recent first)
        bucket.containers.sort_by_key(|c| c.activity_secs);

        if !first_group {
            rows.push(DockerRow::Separator);
        }
        first_group = false;
        let running_count = bucket.containers.iter().filter(|c| c.running).count();
        rows.push(DockerRow::Group {
            name: bucket.name.to_string(),
            path: bucket.path.clone(),
            count: bucket.containers.len(),
            running_count,
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

    // "Other" group always goes last
    if let Some(mut bucket) = other {
        bucket.containers.sort_by_key(|c| c.activity_secs);

        if !rows.is_empty() {
            rows.push(DockerRow::Separator);
        }
        let running_count = bucket.containers.iter().filter(|c| c.running).count();
        rows.push(DockerRow::Group {
            name: bucket.name.to_string(),
            path: bucket.path.clone(),
            count: bucket.containers.len(),
            running_count,
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
    crate::util::apply_filter(containers, filter);
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

fn parse_docker_ports(raw: &str) -> (Cow<'static, str>, Cow<'static, str>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (Cow::Borrowed(DASH), Cow::Borrowed(DASH));
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
        let internal: Cow<'static, str> = if internal_join.is_empty() {
            Cow::Borrowed(DASH)
        } else {
            Cow::Owned(internal_join)
        };
        (Cow::Owned(pub_join), internal)
    } else if !unbound_ports.is_empty() {
        (Cow::Owned(unbound_ports.join(",")), Cow::Borrowed(DASH))
    } else {
        (Cow::Borrowed(DASH), Cow::Borrowed(DASH))
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

/// Parse activity time from Docker status string.
/// Returns seconds since last activity (lower = more recent).
/// Examples:
///   "Up 2 hours" -> ~7200
///   "Up About an hour" -> ~3600
///   "Exited (0) 3 days ago" -> ~259200
///   "Created" -> very high value (least recent)
fn parse_activity_time(status: &str) -> u64 {
    let status_lower = status.to_lowercase();

    // Handle "Created" status (no time info, treat as very old)
    if status_lower.starts_with("created") {
        return u64::MAX / 2;
    }

    // Extract time portion
    // For "Up X time" or "Up About X time"
    // For "Exited (code) X time ago"
    let time_str = if status_lower.starts_with("up") {
        status_lower.trim_start_matches("up").trim()
    } else if let Some(pos) = status_lower.find(')') {
        status_lower[pos + 1..].trim().trim_end_matches("ago").trim()
    } else {
        &status_lower
    };

    parse_duration_string(time_str)
}

/// Parse a duration string like "2 hours", "About an hour", "3 days", "45 seconds"
fn parse_duration_string(input: &str) -> u64 {
    let input = input.trim().to_lowercase();
    let input = input.trim_start_matches("about").trim();

    // Handle special cases
    if input.starts_with("a ") || input.starts_with("an ") {
        // "a minute", "an hour", etc.
        let unit = input.trim_start_matches("a ").trim_start_matches("an ").trim();
        return match unit {
            s if s.starts_with("second") => 1,
            s if s.starts_with("minute") => 60,
            s if s.starts_with("hour") => 3600,
            s if s.starts_with("day") => 86400,
            s if s.starts_with("week") => 604800,
            s if s.starts_with("month") => 2592000,
            s if s.starts_with("year") => 31536000,
            _ => u64::MAX / 2,
        };
    }

    // Parse "N units" format
    let mut parts = input.split_whitespace();
    let number = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
    let unit = parts.next().unwrap_or("");

    let multiplier = match unit {
        s if s.starts_with("second") => 1,
        s if s.starts_with("minute") => 60,
        s if s.starts_with("hour") => 3600,
        s if s.starts_with("day") => 86400,
        s if s.starts_with("week") => 604800,
        s if s.starts_with("month") => 2592000,
        s if s.starts_with("year") => 31536000,
        _ => 1,
    };

    number.saturating_mul(multiplier)
}
