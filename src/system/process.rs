use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use sysinfo::{Pid, System, Uid};

use crate::app::{SortBy, SortOrder};
use crate::util::cmp_f32;

pub struct ProcInfo {
    pub name: String,
    pub name_lower: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub user: String,
    pub parent: Option<Pid>,
    pub is_thread: bool,
}

pub struct TreeRow {
    pub pid: Pid,
    pub prefix: String,
}

#[derive(Clone, PartialEq)]
pub struct ProcessEntry {
    pub pid: Pid,
    pub name: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub user_id: Option<Uid>,
    pub parent: Option<Pid>,
    pub is_thread: bool,
}

/// Static string constant to avoid repeated allocations
const DASH: &str = "-";

pub fn load_process_logs(pid: u32) -> io::Result<String> {
    let cmd = format!(
        "if [ -r /proc/{pid}/fd/1 ] || [ -r /proc/{pid}/fd/2 ]; then \
            echo \"--- stdout (fd/1) ---\"; \
            if [ -r /proc/{pid}/fd/1 ]; then tail -n 200 /proc/{pid}/fd/1; else echo \"fd/1 not readable\"; fi; \
            echo \"\"; \
            echo \"--- stderr (fd/2) ---\"; \
            if [ -r /proc/{pid}/fd/2 ]; then tail -n 200 /proc/{pid}/fd/2; else echo \"fd/2 not readable\"; fi; \
        else \
            if command -v journalctl >/dev/null 2>&1; then \
                echo \"--- journalctl ---\"; \
                journalctl -n 200 _PID={pid} --no-pager; \
            else \
                echo \"No readable stdout/stderr and journalctl not available.\"; \
            fi; \
        fi",
    );

    let output = Command::new("bash").args(["-lc", &cmd]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        if stderr.trim().is_empty() {
            Ok(stdout.to_string())
        } else if stdout.trim().is_empty() {
            Ok(stderr.to_string())
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("log fetch failed: {}", stderr.trim()),
        ))
    }
}

pub fn collect_processes(
    system: &System,
    filter: &str,
    user_cache: &HashMap<Uid, String>,
    skip_threads: bool,
) -> HashMap<Pid, ProcInfo> {
    let filter_lower = filter.to_lowercase();
    let has_filter = !filter_lower.is_empty();

    // Pre-allocate with estimated capacity
    let mut processes: HashMap<Pid, ProcInfo> =
        HashMap::with_capacity(system.processes().len() / 2);

    for (pid, process) in system.processes() {
        if skip_threads && process.thread_kind().is_some() {
            continue;
        }

        // Get name as borrowed reference for filter check
        let name_ref = process.name();

        // Filter early using borrowed reference to avoid allocation for filtered items
        if has_filter {
            // Check if lowercase name contains filter - only allocate if we need to
            let matches = name_ref.to_lowercase().contains(&filter_lower);
            if !matches {
                continue;
            }
        }

        // Only allocate strings after filter passes
        let name = name_ref.to_string();
        let name_lower = name.to_lowercase();
        let is_thread = process.thread_kind().is_some();
        let user = process
            .user_id()
            .and_then(|uid| user_cache.get(uid))
            .cloned()
            .unwrap_or_else(|| DASH.to_string());

        processes.insert(
            *pid,
            ProcInfo {
                name,
                name_lower,
                cpu: process.cpu_usage(),
                memory_bytes: process.memory(),
                user,
                parent: process.parent(),
                is_thread,
            },
        );
    }

    apply_multiprocess_memory_aggregation(&mut processes);

    processes
}

pub fn collect_processes_from_entries(
    entries: &[ProcessEntry],
    filter: &str,
    user_cache: &HashMap<Uid, String>,
    skip_threads: bool,
) -> HashMap<Pid, ProcInfo> {
    let filter_lower = filter.to_lowercase();
    let has_filter = !filter_lower.is_empty();
    let mut processes: HashMap<Pid, ProcInfo> = HashMap::with_capacity(entries.len() / 2);

    for entry in entries {
        if skip_threads && entry.is_thread {
            continue;
        }

        let name_ref = entry.name.as_str();
        if has_filter && !name_ref.to_lowercase().contains(&filter_lower) {
            continue;
        }

        let name = name_ref.to_string();
        let name_lower = name.to_lowercase();
        let user = entry
            .user_id
            .as_ref()
            .and_then(|uid| user_cache.get(uid))
            .cloned()
            .unwrap_or_else(|| DASH.to_string());

        processes.insert(
            entry.pid,
            ProcInfo {
                name,
                name_lower,
                cpu: entry.cpu,
                memory_bytes: entry.memory_bytes,
                user,
                parent: entry.parent,
                is_thread: entry.is_thread,
            },
        );
    }

    processes
}

pub struct ProcessWorker {
    data: Arc<RwLock<Arc<Vec<ProcessEntry>>>>,
    paused: Arc<AtomicBool>,
}

impl ProcessWorker {
    pub fn snapshot(&self) -> Arc<Vec<ProcessEntry>> {
        let guard = self.data.read().unwrap_or_else(|err| err.into_inner());
        Arc::clone(&guard)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

pub fn start_process_worker(interval: Duration) -> ProcessWorker {
    let data = Arc::new(RwLock::new(Arc::new(Vec::new())));
    let thread_data = Arc::clone(&data);
    let paused = Arc::new(AtomicBool::new(false));
    let thread_paused = Arc::clone(&paused);

    std::thread::spawn(move || {
        let mut system = System::new();
        loop {
            if thread_paused.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                continue;
            }
            system.refresh_processes();
            system.refresh_cpu();

            let mut entries = Vec::with_capacity(system.processes().len());
            for (pid, process) in system.processes() {
                entries.push(ProcessEntry {
                    pid: *pid,
                    name: process.name().to_string(),
                    cpu: process.cpu_usage(),
                    memory_bytes: process.memory(),
                    user_id: process.user_id().cloned(),
                    parent: process.parent(),
                    is_thread: process.thread_kind().is_some(),
                });
            }
            let should_update = {
                let guard = thread_data.read().unwrap_or_else(|err| err.into_inner());
                guard.as_ref() != &entries
            };
            if should_update {
                let mut guard = thread_data.write().unwrap_or_else(|err| err.into_inner());
                *guard = Arc::new(entries);
            }
            std::thread::sleep(interval);
        }
    });

    ProcessWorker { data, paused }
}

pub fn build_tree_rows(
    processes: &HashMap<Pid, ProcInfo>,
    sort_by: SortBy,
    sort_order: SortOrder,
    show_children: bool,
) -> Vec<TreeRow> {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    let mut roots: Vec<Pid> = Vec::new();

    for (pid, info) in processes {
        let mut is_child = false;
        if let Some(parent) = info.parent {
            let has_parent = parent != *pid && processes.contains_key(&parent);
            let skipped_parent = has_parent && is_skipped_parent(parent, processes);
            if show_children && has_parent && !skipped_parent {
                children.entry(parent).or_default().push(*pid);
                is_child = true;
            } else if has_parent && !skipped_parent {
                is_child = true;
            }
        }

        if !is_child {
            roots.push(*pid);
        }
    }

    sort_pid_list(&mut roots, processes, sort_by, sort_order);
    if show_children {
        for list in children.values_mut() {
            sort_pid_list(list, processes, sort_by, sort_order);
        }
    }

    let mut rows = Vec::new();
    if show_children {
        let mut ancestor_last = Vec::new();
        for (idx, pid) in roots.iter().enumerate() {
            let is_last = idx + 1 == roots.len();
            push_tree_rows(*pid, is_last, &mut ancestor_last, &children, &mut rows);
        }
    } else {
        for pid in roots {
            rows.push(TreeRow {
                pid,
                prefix: String::new(),
            });
        }
    }

    rows
}

pub fn load_process_env(pid: Pid) -> io::Result<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{}/environ", pid.as_u32());
        let bytes = std::fs::read(path)?;
        let mut vars = Vec::new();
        for entry in bytes.split(|byte| *byte == 0u8) {
            if entry.is_empty() {
                continue;
            }
            vars.push(String::from_utf8_lossy(entry).to_string());
        }
        if vars.is_empty() {
            vars.push("No env vars found".to_string());
        }
        Ok(vars)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Other,
            "process env only supported on Linux",
        ))
    }
}

fn sort_pid_list(
    pids: &mut [Pid],
    processes: &HashMap<Pid, ProcInfo>,
    sort_by: SortBy,
    sort_order: SortOrder,
) {
    pids.sort_by(|a_pid, b_pid| {
        let ordering = match (processes.get(a_pid), processes.get(b_pid)) {
            (Some(a), Some(b)) => compare_proc(a, b, sort_by),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        if ordering == std::cmp::Ordering::Equal {
            a_pid.cmp(b_pid)
        } else {
            ordering
        }
    });

    if sort_order == SortOrder::Desc {
        pids.reverse();
    }
}

fn compare_proc(a: &ProcInfo, b: &ProcInfo, sort_by: SortBy) -> std::cmp::Ordering {
    match sort_by {
        SortBy::Cpu => cmp_f32(a.cpu, b.cpu),
        SortBy::Memory => a.memory_bytes.cmp(&b.memory_bytes),
        SortBy::Name => a.name_lower.cmp(&b.name_lower),
    }
}

fn push_tree_rows(
    pid: Pid,
    is_last: bool,
    ancestor_last: &mut Vec<bool>,
    children: &HashMap<Pid, Vec<Pid>>,
    rows: &mut Vec<TreeRow>,
) {
    let prefix = build_tree_prefix(ancestor_last, is_last);
    rows.push(TreeRow { pid, prefix });

    ancestor_last.push(is_last);
    if let Some(child_list) = children.get(&pid) {
        for (idx, child_pid) in child_list.iter().enumerate() {
            let child_last = idx + 1 == child_list.len();
            push_tree_rows(*child_pid, child_last, ancestor_last, children, rows);
        }
    }
    ancestor_last.pop();
}

fn build_tree_prefix(ancestor_last: &[bool], is_last: bool) -> String {
    if ancestor_last.is_empty() {
        return String::new();
    }

    let mut prefix = String::new();
    for &last in ancestor_last {
        if last {
            prefix.push_str("   ");
        } else {
            prefix.push_str("│  ");
        }
    }

    if is_last {
        prefix.push_str("└─ ");
    } else {
        prefix.push_str("├─ ");
    }

    prefix
}

fn is_skipped_parent(pid: Pid, processes: &HashMap<Pid, ProcInfo>) -> bool {
    if pid == Pid::from_u32(1) {
        return true;
    }

    matches!(
        processes.get(&pid).map(|proc_info| proc_info.name.as_str()),
        Some("gnome-shell")
    )
}

/// Aggregates memory for multi-process applications (Chrome, Firefox, Electron apps).
/// Groups all processes by app family and sums PSS for accurate memory reporting.
fn apply_multiprocess_memory_aggregation(processes: &mut HashMap<Pid, ProcInfo>) {
    // Group processes by their app family
    // Each family is identified by a key (e.g., "chrome", "firefox", "code")
    let mut app_families: HashMap<&'static str, Vec<Pid>> = HashMap::new();

    for (pid, info) in processes.iter() {
        if info.is_thread {
            continue;
        }
        if let Some(family) = get_app_family(&info.name_lower) {
            app_families.entry(family).or_default().push(*pid);
        }
    }

    // For each app family, find the root process and sum all family members' memory
    let mut mem_cache: HashMap<Pid, u64> = HashMap::new();

    for (family, pids) in app_families.iter() {
        if pids.is_empty() {
            continue;
        }

        // Find the root process (the one whose parent is not in this family)
        let root_pid = find_family_root(pids, processes, family);

        // Try cgroup memory only if all processes share the same cgroup
        // Chrome/Chromium often spawn child processes in different cgroup scopes,
        // so we must verify they're all in the same cgroup before using cgroup memory.
        let total = if let Some(cgroup_mem) = read_cgroup_memory_if_unified(pids, family) {
            cgroup_mem
        } else {
            // Sum PSS for all processes in this family
            let mut total: u64 = 0;
            for pid in pids {
                let mem = memory_for_pid(*pid, processes, &mut mem_cache);
                total = total.saturating_add(mem);
            }
            total
        };

        // Set the aggregated memory on the root process
        if let Some(info) = processes.get_mut(&root_pid) {
            info.memory_bytes = total;
        }
    }
}

/// Returns the app family identifier for known multi-process applications.
fn get_app_family(name_lower: &str) -> Option<&'static str> {
    // Chrome and Chromium
    if name_lower.contains("chrome") || name_lower.contains("chromium") {
        return Some("chrome");
    }
    // Firefox
    if name_lower.contains("firefox") || name_lower.contains("geckodriver") {
        return Some("firefox");
    }
    // JetBrains IDEs
    if name_lower.contains("idea")
        || name_lower.contains("webstorm")
        || name_lower.contains("phpstorm")
        || name_lower.contains("pycharm")
        || name_lower.contains("rubymine")
        || name_lower.contains("goland")
        || name_lower.contains("clion")
        || name_lower.contains("rider")
        || name_lower.contains("datagrip")
        || name_lower.contains("dataspell")
        || name_lower.contains("appcode")
        || name_lower.contains("android-studio")
    {
        return Some("jetbrains");
    }
    // VS Code
    if name_lower.contains("code") && !name_lower.contains("codec") {
        return Some("vscode");
    }
    // Electron apps (each gets its own family)
    if name_lower.contains("slack") {
        return Some("slack");
    }
    if name_lower.contains("discord") {
        return Some("discord");
    }
    if name_lower.contains("spotify") {
        return Some("spotify");
    }
    if name_lower.contains("teams") {
        return Some("teams");
    }
    if name_lower.contains("obsidian") {
        return Some("obsidian");
    }
    if name_lower.contains("notion") {
        return Some("notion");
    }
    if name_lower.contains("postman") {
        return Some("postman");
    }
    if name_lower.contains("figma") {
        return Some("figma");
    }
    if name_lower.contains("gitkraken") {
        return Some("gitkraken");
    }
    if name_lower.contains("insomnia") {
        return Some("insomnia");
    }
    // Eclipse and NetBeans
    if name_lower.contains("eclipse") {
        return Some("eclipse");
    }
    if name_lower.contains("netbeans") {
        return Some("netbeans");
    }
    None
}

/// Finds the root process of an app family.
/// Returns the process with the lowest PID whose parent is NOT in the same family.
/// This ensures deterministic selection (main process usually has lowest PID).
fn find_family_root(pids: &[Pid], processes: &HashMap<Pid, ProcInfo>, family: &str) -> Pid {
    // Find all processes whose parent is NOT in the same family (potential roots)
    let mut candidates: Vec<Pid> = pids
        .iter()
        .filter(|pid| {
            processes.get(pid).map_or(false, |info| {
                let parent_in_family = info
                    .parent
                    .and_then(|parent| processes.get(&parent))
                    .map(|parent_info| get_app_family(&parent_info.name_lower) == Some(family))
                    .unwrap_or(false);
                !parent_in_family
            })
        })
        .copied()
        .collect();

    // Sort by PID and return the lowest (main process usually has lowest PID)
    candidates.sort();
    candidates.first().copied().unwrap_or_else(|| {
        // Fallback: return the lowest PID from all family members
        pids.iter().min().copied().unwrap_or(Pid::from_u32(0))
    })
}

fn memory_for_pid(
    pid: Pid,
    processes: &HashMap<Pid, ProcInfo>,
    mem_cache: &mut HashMap<Pid, u64>,
) -> u64 {
    if let Some(value) = mem_cache.get(&pid) {
        return *value;
    }
    let fallback = match processes.get(&pid) {
        Some(proc_info) if proc_info.is_thread => return 0,
        Some(proc_info) => proc_info.memory_bytes,
        None => 0,
    };
    let value = read_pss_bytes_cached(pid).unwrap_or(fallback);
    mem_cache.insert(pid, value);
    value
}

#[derive(Clone)]
struct PssEntry {
    value: Option<u64>,
    updated: Instant,
}

const PSS_TTL: Duration = Duration::from_secs(10);

fn read_pss_bytes_cached(pid: Pid) -> Option<u64> {
    let cache = pss_cache();
    let now = Instant::now();
    if let Some(entry) = cache.lock().ok().and_then(|guard| guard.get(&pid).cloned()) {
        if now.duration_since(entry.updated) < PSS_TTL {
            return entry.value;
        }
    }

    let value = read_pss_bytes(pid);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(
            pid,
            PssEntry {
                value,
                updated: now,
            },
        );
    }
    value
}

fn pss_cache() -> &'static Mutex<HashMap<Pid, PssEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<Pid, PssEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_os = "linux")]
fn read_pss_bytes(pid: Pid) -> Option<u64> {
    let path = format!("/proc/{}/smaps_rollup", pid.as_u32());
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line.ok()?;
        if let Some(rest) = line.strip_prefix("Pss:") {
            let mut parts = rest.split_whitespace();
            let value = parts.next()?.parse::<u64>().ok()?;
            return Some(value.saturating_mul(1024));
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_pss_bytes(_pid: Pid) -> Option<u64> {
    None
}

/// Read cgroup memory only if all processes in the family share the same cgroup.
/// Returns None if processes are spread across multiple cgroups (common with Chrome).
#[cfg(target_os = "linux")]
fn read_cgroup_memory_if_unified(pids: &[Pid], app_name_lower: &str) -> Option<u64> {
    if pids.is_empty() {
        return None;
    }

    // Get the cgroup path for the first process
    let first_cgroup = get_cgroup_path(pids[0])?;

    // Check that all other processes are in the same cgroup
    for pid in pids.iter().skip(1) {
        if let Some(cgroup) = get_cgroup_path(*pid) {
            if cgroup != first_cgroup {
                // Processes are in different cgroups, fall back to PSS summing
                return None;
            }
        }
    }

    // All processes share the same cgroup, read its memory
    let path_lower = first_cgroup.to_lowercase();
    let is_app_specific = path_lower.contains(app_name_lower)
        || path_lower.contains("app-")
        || (path_lower.contains(".scope") && !path_lower.contains("session"));

    if !is_app_specific {
        return None;
    }

    let mem_path = format!("/sys/fs/cgroup{}/memory.current", first_cgroup);
    std::fs::read_to_string(&mem_path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Get the cgroup v2 path for a process.
#[cfg(target_os = "linux")]
fn get_cgroup_path(pid: Pid) -> Option<String> {
    let cgroup_file = format!("/proc/{}/cgroup", pid.as_u32());
    let file = File::open(cgroup_file).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line.ok()?;
        if line.starts_with("0::") {
            return line.strip_prefix("0::").map(|s| s.to_string());
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_cgroup_memory_if_unified(_pids: &[Pid], _app_name_lower: &str) -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
fn get_cgroup_path(_pid: Pid) -> Option<String> {
    None
}
