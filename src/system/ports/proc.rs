use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sysinfo::{Pid, System};

use super::PortInfo;

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

pub fn collect_proc_ports(system: &System, inode_map: &HashMap<u64, Pid>) -> Vec<PortInfo> {
    // Pre-allocate with reasonable capacity
    let mut rows = Vec::with_capacity(64);

    parse_socket_table(
        "/proc/net/tcp",
        "tcp",
        Some("0A"),
        inode_map,
        system,
        &mut rows,
    );
    parse_socket_table(
        "/proc/net/tcp6",
        "tcp6",
        Some("0A"),
        inode_map,
        system,
        &mut rows,
    );
    parse_socket_table("/proc/net/udp", "udp", None, inode_map, system, &mut rows);
    parse_socket_table("/proc/net/udp6", "udp6", None, inode_map, system, &mut rows);

    rows
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
fn build_inode_pid_map_uncached() -> HashMap<u64, Pid> {
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
                    map.entry(inode).or_insert(pid);
                }
            }
        }
    }

    map
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

fn parse_socket_inode(path: &Path) -> Option<u64> {
    let link = path.to_string_lossy();
    if !link.starts_with("socket:[") || !link.ends_with(']') {
        return None;
    }
    let inner = link.trim_start_matches("socket:[").trim_end_matches(']');
    inner.parse::<u64>().ok()
}
