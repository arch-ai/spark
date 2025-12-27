use std::collections::HashMap;

use sysinfo::{Pid, System, Uid};

use crate::app::{SortBy, SortOrder};
use super::docker;
use crate::util::cmp_f32;

pub struct ProcInfo {
    pub pid: Pid,
    pub name: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub user: String,
    pub exe_path: String,
    pub parent: Option<Pid>,
    pub container: Option<String>,
}

pub struct TreeRow {
    pub pid: Pid,
    pub prefix: String,
}

pub fn collect_processes(
    system: &System,
    filter: &str,
    container_cache: &HashMap<String, String>,
    user_cache: &HashMap<Uid, String>,
    skip_threads: bool,
) -> HashMap<Pid, ProcInfo> {
    let filter_lower = filter.to_lowercase();
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            if skip_threads && process.thread_kind().is_some() {
                return None;
            }

            let name = process.name().to_string();
            if !filter_lower.is_empty() && !name.to_lowercase().contains(&filter_lower) {
                return None;
            }

            let container = docker::container_label_for(*pid, container_cache);
            let user = process
                .user_id()
                .and_then(|uid| user_cache.get(uid))
                .cloned()
                .unwrap_or_else(|| "-".to_string());

            Some(ProcInfo {
                pid: *pid,
                name,
                cpu: process.cpu_usage(),
                memory_bytes: process.memory(),
                user,
                exe_path: process
                    .exe()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "-".to_string()),
                parent: process.parent(),
                container,
            })
        })
        .map(|info| (info.pid, info))
        .collect()
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
        SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
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
