use std::collections::HashMap;
use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

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
