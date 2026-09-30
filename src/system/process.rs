use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use sysinfo::{Pid, System, Uid};

use crate::app::{SortBy, SortOrder};
use crate::util::cmp_f32;

mod memory;
pub use memory::MemorySample;

pub struct ProcInfo {
    pub name: String,
    pub name_lower: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub memory_estimated: bool,
    pub tree_memory_bytes: u64,
    pub tree_memory_estimated: bool,
    pub tree_swap_bytes: Option<u64>,
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
    pub start_time: u64,
    pub memory_sample: Option<MemorySample>,
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
) -> HashMap<Pid, ProcInfo> {
    let filter_lower = filter.to_lowercase();
    let has_filter = !filter_lower.is_empty();
    let mut processes: HashMap<Pid, ProcInfo> = HashMap::with_capacity(entries.len() / 2);

    for entry in entries {
        // Threads share the process address space; counting them inflates totals.
        if entry.is_thread {
            continue;
        }

        let name_ref = entry.name.as_str();

        let name = name_ref.to_string();
        let name_lower = name.to_lowercase();
        let user = entry
            .user_id
            .as_ref()
            .and_then(|uid| user_cache.get(uid))
            .cloned()
            .unwrap_or_else(|| DASH.to_string());

        let memory_bytes = entry
            .memory_sample
            .map_or(entry.memory_bytes, |sample| sample.pss_bytes);
        let memory_estimated = entry.memory_sample.is_none();
        processes.insert(
            entry.pid,
            ProcInfo {
                name,
                name_lower,
                cpu: entry.cpu,
                memory_bytes,
                memory_estimated,
                tree_memory_bytes: memory_bytes,
                tree_memory_estimated: memory_estimated,
                tree_swap_bytes: entry.memory_sample.and_then(|sample| sample.swap_pss_bytes),
                user,
                parent: entry.parent,
            },
        );
    }

    aggregate_tree_memory(&mut processes);
    // Searching for a parent must not discard memory used by differently named children.
    if has_filter {
        processes.retain(|_, info| info.name_lower.contains(&filter_lower));
    }
    processes
}

fn aggregate_tree_memory(processes: &mut HashMap<Pid, ProcInfo>) {
    let parents: HashMap<_, _> = processes
        .iter()
        .filter_map(|(pid, info)| {
            info.parent
                .filter(|parent| {
                    parent != pid
                        && processes.contains_key(parent)
                        && !is_skipped_parent(*parent, processes)
                })
                .map(|parent| (*pid, parent))
        })
        .collect();
    let mut remaining: HashMap<Pid, usize> = processes.keys().map(|pid| (*pid, 0)).collect();
    for parent in parents.values() {
        *remaining.get_mut(parent).unwrap() += 1;
    }
    let mut ready: VecDeque<_> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(pid, _)| *pid)
        .collect();
    while let Some(pid) = ready.pop_front() {
        let Some(parent) = parents.get(&pid) else {
            continue;
        };
        let info = &processes[&pid];
        let (memory, estimated, swap) = (
            info.tree_memory_bytes,
            info.tree_memory_estimated,
            info.tree_swap_bytes,
        );
        let parent_info = processes.get_mut(parent).unwrap();
        parent_info.tree_memory_bytes = parent_info.tree_memory_bytes.saturating_add(memory);
        parent_info.tree_memory_estimated |= estimated;
        parent_info.tree_swap_bytes = parent_info
            .tree_swap_bytes
            .zip(swap)
            .map(|(a, b)| a.saturating_add(b));
        let count = remaining.get_mut(parent).unwrap();
        *count -= 1;
        if *count == 0 {
            ready.push_back(*parent);
        }
    }
    for (pid, info) in processes {
        // Break inconsistent parent cycles from a changing process snapshot.
        // Each cycle member keeps only its own memory and its acyclic descendants.
        info.parent = if remaining[pid] > 0 {
            None
        } else {
            parents.get(pid).copied()
        };
    }
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
        let mut memory = memory::MemorySampler::default();
        loop {
            if thread_paused.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                continue;
            }
            system.refresh_processes();
            system.refresh_cpu();

            let mut entries = Vec::with_capacity(system.processes().len());
            for (pid, process) in system.processes() {
                if process.thread_kind().is_some() {
                    continue;
                }
                entries.push(ProcessEntry {
                    pid: *pid,
                    name: process.name().to_string(),
                    cpu: process.cpu_usage(),
                    memory_bytes: process.memory(),
                    start_time: process.start_time(),
                    memory_sample: None,
                    user_id: process.user_id().cloned(),
                    parent: process.parent(),
                    is_thread: process.thread_kind().is_some(),
                });
            }
            memory.refresh(&mut entries);
            entries.sort_unstable_by_key(|entry| entry.pid);
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
        SortBy::Memory => a.tree_memory_bytes.cmp(&b.tree_memory_bytes),
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

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn entry(pid: u32, parent: Option<u32>, name: &str, rss: u64) -> ProcessEntry {
        ProcessEntry {
            pid: Pid::from_u32(pid),
            parent: parent.map(Pid::from_u32),
            name: name.into(),
            cpu: 0.0,
            memory_bytes: rss,
            memory_sample: None,
            start_time: 1,
            user_id: None,
            is_thread: false,
        }
    }

    fn measured(mut entry: ProcessEntry, pss: u64, swap: u64) -> ProcessEntry {
        entry.memory_sample = Some(MemorySample {
            pss_bytes: pss,
            swap_pss_bytes: Some(swap),
        });
        entry
    }

    #[test]
    fn tree_totals_use_proportional_memory_and_ignore_shared_threads() {
        let mut thread = entry(13, Some(10), "thread", 9999);
        thread.is_thread = true;
        let entries = vec![
            measured(entry(10, None, "chrome", 500), 200, 10),
            measured(entry(11, Some(10), "renderer", 400), 250, 20),
            measured(entry(12, Some(11), "worker", 300), 150, 5),
            thread,
            measured(entry(20, None, "editor", 450), 450, 0),
        ];
        let procs = collect_processes_from_entries(&entries, "", &HashMap::new());
        let chrome = &procs[&Pid::from_u32(10)];
        assert_eq!(chrome.memory_bytes, 200);
        assert_eq!(chrome.tree_memory_bytes, 600);
        assert_eq!(chrome.tree_swap_bytes, Some(35));
        assert!(!chrome.tree_memory_estimated);
        assert!(!procs.contains_key(&Pid::from_u32(13)));
        for expanded in [true, false] {
            let rows = build_tree_rows(&procs, SortBy::Memory, SortOrder::Desc, expanded);
            assert_eq!(rows[0].pid, Pid::from_u32(10));
        }
        let filtered = collect_processes_from_entries(&entries, "chrome", &HashMap::new());
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[&Pid::from_u32(10)].tree_memory_bytes, 600);
    }

    #[test]
    fn rss_fallback_marks_the_entire_tree_and_keeps_swap_unknown() {
        let entries = vec![
            measured(entry(10, None, "app", 500), 200, 10),
            entry(11, Some(10), "restricted-child", 400),
        ];
        let procs = collect_processes_from_entries(&entries, "", &HashMap::new());
        let root = &procs[&Pid::from_u32(10)];
        assert_eq!(root.tree_memory_bytes, 600);
        assert!(!root.memory_estimated);
        assert!(root.tree_memory_estimated);
        assert!(root.tree_swap_bytes.is_none());
    }

    #[test]
    fn tree_boundaries_cycles_and_large_values_cannot_inflate_or_overflow_totals() {
        let entries = vec![
            entry(1, None, "init", 10),
            entry(2, Some(1), "app", 20),
            entry(3, Some(99), "orphan", 30),
            entry(4, Some(5), "cycle-a", 40),
            entry(5, Some(4), "cycle-b", 50),
            entry(6, Some(4), "cycle-child", 60),
            entry(7, None, "large", u64::MAX),
            entry(8, Some(7), "large-child", 1),
        ];
        let procs = collect_processes_from_entries(&entries, "", &HashMap::new());
        assert_eq!(procs[&Pid::from_u32(1)].tree_memory_bytes, 10);
        assert_eq!(procs[&Pid::from_u32(4)].tree_memory_bytes, 100);
        assert_eq!(procs[&Pid::from_u32(5)].tree_memory_bytes, 50);
        assert_eq!(procs[&Pid::from_u32(7)].tree_memory_bytes, u64::MAX);
        assert_eq!(
            build_tree_rows(&procs, SortBy::Memory, SortOrder::Desc, true).len(),
            entries.len()
        );
    }
}
