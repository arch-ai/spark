use std::collections::HashMap;
use std::env;
use std::io;
use std::process::Command;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use sysinfo::Pid;

use crate::app::{SortBy, SortOrder};
use crate::util::cmp_f32;

#[derive(Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub port_public: String,
    pub port_internal: String,
    pub status: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub group_name: String,
    pub group_path: Option<String>,
}

#[derive(Clone)]
pub enum DockerRow {
    Group {
        name: String,
        path: Option<String>,
        count: usize,
    },
    Item { index: usize, prefix: String },
    Separator,
}

#[derive(Clone)]
struct ComposeGroup {
    name: String,
    path: Option<String>,
}

pub struct DockerStatsWorker {
    data: Arc<Mutex<Vec<ContainerInfo>>>,
}

impl DockerStatsWorker {
    pub fn snapshot(&self) -> Vec<ContainerInfo> {
        let guard = self.data.lock().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    }
}

pub fn start_docker_stats_worker(interval: Duration) -> DockerStatsWorker {
    let data = Arc::new(Mutex::new(Vec::new()));
    let thread_data = Arc::clone(&data);

    thread::spawn(move || loop {
        if let Some(stats) = load_docker_stats() {
            let mut guard = thread_data.lock().unwrap_or_else(|err| err.into_inner());
            *guard = stats;
        }
        thread::sleep(interval);
    });

    DockerStatsWorker { data }
}

pub fn open_container_shell(container_id: &str) -> io::Result<()> {
    let cmd = format!(
        "docker exec -it {id} bash 2>/dev/null || docker exec -it {id} sh; exec bash",
        id = container_id
    );
    if let Ok(term) = env::var("TERMINAL") {
        if try_spawn_terminal(&term, TerminalMode::DashE, &cmd).is_ok() {
            return Ok(());
        }
    }

    let mut last_err = None;
    let candidates = [
        ("x-terminal-emulator", TerminalMode::DashE),
        ("gnome-terminal", TerminalMode::DoubleDash),
        ("konsole", TerminalMode::DashE),
        ("xfce4-terminal", TerminalMode::DashE),
        ("mate-terminal", TerminalMode::DoubleDash),
        ("tilix", TerminalMode::DashE),
        ("xterm", TerminalMode::DashE),
    ];

    for (name, mode) in candidates {
        match try_spawn_terminal(name, mode, &cmd) {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "No supported terminal found")
    }))
}

pub fn open_container_logs(container_id: &str) -> io::Result<()> {
    let cmd = format!("docker logs -f --tail 200 {id}; exec bash", id = container_id);
    if let Ok(term) = env::var("TERMINAL") {
        if try_spawn_terminal(&term, TerminalMode::DashE, &cmd).is_ok() {
            return Ok(());
        }
    }

    let mut last_err = None;
    let candidates = [
        ("x-terminal-emulator", TerminalMode::DashE),
        ("gnome-terminal", TerminalMode::DoubleDash),
        ("konsole", TerminalMode::DashE),
        ("xfce4-terminal", TerminalMode::DashE),
        ("mate-terminal", TerminalMode::DoubleDash),
        ("tilix", TerminalMode::DashE),
        ("xterm", TerminalMode::DashE),
    ];

    for (name, mode) in candidates {
        match try_spawn_terminal(name, mode, &cmd) {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "No supported terminal found")
    }))
}

pub fn load_container_env(container_id: &str) -> io::Result<Vec<String>> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
            container_id,
        ])
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "docker inspect failed",
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<String> = stdout
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    if lines.is_empty() {
        lines.push("No env vars found".to_string());
    }
    Ok(lines)
}

pub fn kill_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["kill", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "docker kill failed",
        ))
    }
}

enum TerminalMode {
    DashE,
    DoubleDash,
}

fn try_spawn_terminal(term: &str, mode: TerminalMode, cmd: &str) -> io::Result<()> {
    let mut command = Command::new(term);
    match mode {
        TerminalMode::DashE => {
            command.args(["-e", "bash", "-lc", cmd]);
        }
        TerminalMode::DoubleDash => {
            command.args(["--", "bash", "-lc", cmd]);
        }
    }
    command.spawn().map(|_| ())
}

pub fn load_docker_container_cache() -> Option<HashMap<String, String>> {
    let output = Command::new("docker")
        .args(["ps", "--no-trunc", "--format", "{{.ID}} {{.Names}}"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut cache = HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(2, ' ');
        let id = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        if id.is_empty() || name.is_empty() {
            continue;
        }
        cache.insert(id.to_string(), name.to_string());
        if id.len() >= 12 {
            cache.insert(id[..12].to_string(), name.to_string());
        }
    }

    Some(cache)
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

pub fn container_label_for(pid: Pid, container_cache: &HashMap<String, String>) -> Option<String> {
    let id = get_container_id(pid)?;
    if let Some(name) = resolve_container_name(&id, container_cache) {
        return Some(name);
    }

    let short = if id.len() > 12 { &id[..12] } else { &id };
    Some(format!("ctr:{short}"))
}

fn resolve_container_name(id: &str, container_cache: &HashMap<String, String>) -> Option<String> {
    if let Some(name) = container_cache.get(id) {
        return Some(name.clone());
    }
    if id.len() > 12 {
        if let Some(name) = container_cache.get(&id[..12]) {
            return Some(name.clone());
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn get_container_id(pid: Pid) -> Option<String> {
    let path = format!("/proc/{}/cgroup", pid.as_u32());
    let contents = std::fs::read_to_string(path).ok()?;
    extract_container_id(&contents)
}

#[cfg(not(target_os = "linux"))]
fn get_container_id(_pid: Pid) -> Option<String> {
    None
}

fn extract_container_id(cgroup: &str) -> Option<String> {
    let mut best = String::new();
    for line in cgroup.lines() {
        let mut current = String::new();
        for ch in line.chars() {
            if ch.is_ascii_hexdigit() {
                current.push(ch);
            } else {
                if current.len() >= 12 && current.len() > best.len() {
                    best = current.clone();
                }
                current.clear();
            }
        }
        if current.len() >= 12 && current.len() > best.len() {
            best = current;
        }
    }

    if best.is_empty() {
        None
    } else {
        Some(best)
    }
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
