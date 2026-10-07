use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sysinfo::{Pid, System};

use super::PortInfo;
use crate::system::node;

/// Cached inode-to-PID map with TTL to reduce /proc scanning overhead.
/// The map is rebuilt when it expires or when explicitly invalidated.
struct InodeMapCache {
    map: HashMap<u64, Pid>,
    last_refresh: Instant,
}

const INODE_MAP_TTL: Duration = Duration::from_secs(2);

fn inode_map_cache() -> &'static Mutex<Option<InodeMapCache>> {
    static CACHE: OnceLock<Mutex<Option<InodeMapCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn collect_proc_ports(
    system: &System,
    inode_map: &HashMap<u64, Pid>,
) -> std::io::Result<Vec<PortInfo>> {
    // Pre-allocate with reasonable capacity
    let mut rows = Vec::with_capacity(64);

    for (path, proto, state) in [
        ("/proc/net/tcp", "tcp", "0A"),
        ("/proc/net/tcp6", "tcp6", "0A"),
        ("/proc/net/udp", "udp", "07"),
        ("/proc/net/udp6", "udp6", "07"),
    ] {
        match fs::read_to_string(path) {
            Ok(contents) => {
                parse_socket_contents(&contents, proto, Some(state), inode_map, system, &mut rows)
            }
            Err(err) if proto.ends_with('6') && err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(std::io::Error::new(
                    err.kind(),
                    format!("Cannot read listening ports: {err}"),
                ))
            }
        }
    }

    Ok(rows)
}

/// Build inode-to-PID map with caching.
/// Caches the result for INODE_MAP_TTL to avoid expensive /proc scanning on every call.
pub fn build_inode_pid_map() -> HashMap<u64, Pid> {
    let cache = inode_map_cache();

    // Check cache first
    if let Ok(guard) = cache.lock() {
        if let Some(ref cached) = *guard {
            if cached.last_refresh.elapsed() < INODE_MAP_TTL {
                return cached.map.clone();
            }
        }
    }

    // Cache miss or expired - rebuild the map
    let map = build_inode_pid_map_uncached();

    // Update cache
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(InodeMapCache {
            map: map.clone(),
            last_refresh: Instant::now(),
        });
    }

    map
}

/// Build the inode-to-PID map without caching.
/// Scans /proc/*/fd/* to find socket inodes.
pub(super) fn build_inode_pid_map_uncached() -> HashMap<u64, Pid> {
    let mut map = HashMap::with_capacity(1024);
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
                    map.entry(inode)
                        .and_modify(|owner: &mut Pid| {
                            *owner = (*owner).min(pid);
                        })
                        .or_insert(pid);
                }
            }
        }
    }

    map
}

fn parse_socket_contents(
    contents: &str,
    proto: &str,
    state_filter: Option<&str>,
    inode_map: &HashMap<u64, Pid>,
    system: &System,
    out: &mut Vec<PortInfo>,
) {
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
        let pid = inode_map
            .get(&inode)
            .copied()
            .unwrap_or_else(|| Pid::from_u32(0));
        let process = system.process(pid);
        let name = process
            .map(|process| process.name().to_string())
            .unwrap_or_else(|| "Unknown owner".into());
        let exe_path = process
            .and_then(|process| process.exe())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Owner unavailable or inaccessible".into());
        let project_name = process.and_then(node::project_name_from_process);

        out.push(PortInfo {
            proto: proto.to_string(),
            port,
            internal_port: None,
            pid,
            name,
            exe_path,
            container_id: None,
            group_name: None,
            project_name,
        });
    }
}

fn parse_port(local: &str) -> u16 {
    let mut parts = local.split(':');
    parts.next();
    let port_hex = parts.next().unwrap_or("");
    u16::from_str_radix(port_hex, 16).unwrap_or(0)
}

fn parse_socket_inode(path: &Path) -> Option<u64> {
    let link = path.to_string_lossy();
    if !link.starts_with("socket:[") || !link.ends_with(']') {
        return None;
    }
    let inner = link.trim_start_matches("socket:[").trim_end_matches(']');
    inner.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inaccessible_owners_remain_visible_and_connected_udp_is_excluded() {
        let contents = "header\n0: 0100007F:1F90 00000000:0000 07 0:0 0:0 0 1000 0 123\n1: 0100007F:1F91 0100007F:0035 01 0:0 0:0 0 1000 0 124\n";
        let mut rows = Vec::new();
        parse_socket_contents(
            contents,
            "udp",
            Some("07"),
            &HashMap::new(),
            &System::new(),
            &mut rows,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].port, 8080);
        assert_eq!(rows[0].pid.as_u32(), 0);
        assert_eq!(rows[0].name, "Unknown owner");
    }
}
