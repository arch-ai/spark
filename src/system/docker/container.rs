use std::collections::HashMap;
use std::io;
use std::process::Command;

use sysinfo::Pid;

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

pub fn kill_containers(container_ids: &[String]) -> (usize, usize) {
    let mut success = 0;
    let mut failed = 0;
    for id in container_ids {
        match kill_container(id) {
            Ok(()) => success += 1,
            Err(_) => failed += 1,
        }
    }
    (success, failed)
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
