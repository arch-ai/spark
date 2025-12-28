mod detect;
mod pm2;

use std::collections::HashMap;

use sysinfo::Pid;

pub use detect::detect_node_processes;
pub(crate) use detect::project_name_from_process;
pub use pm2::{is_pm2_running, load_pm2_processes, Pm2Process};

/// Information about a Node.js process, optionally enriched with PM2 data.
#[derive(Clone, Debug)]
pub struct NodeProcessInfo {
    pub pid: Pid,
    pub name: String,
    pub script: String,
    pub project_name: Option<String>,
    pub uses_nvm: bool,
    pub node_version: Option<String>,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub uptime_secs: Option<u64>,
    /// PM2-specific fields (None if not managed by PM2)
    pub pm2: Option<Pm2Info>,
    /// Number of worker processes (for cluster mode, 1 = single process)
    pub worker_count: usize,
}

/// PM2-specific information for a process.
#[derive(Clone, Debug)]
pub struct Pm2Info {
    pub pm_id: u32,
    pub name: String,
    pub mode: String,       // "fork" or "cluster"
    pub status: String,     // "online", "stopped", "errored"
    pub restarts: u32,
    pub pm2_uptime: Option<u64>,
}

pub enum NodeRow {
    Group { name: String, count: usize },
    Item { index: usize },
    UtilsSpacer,
    UtilsTitle,
    UtilsTop,
    UtilsHeader,
    UtilsSeparator,
}

/// Collect all Node.js processes, merging with PM2 data if available.
/// Groups cluster workers together to avoid duplicates.
pub fn collect_node_processes(
    system: &sysinfo::System,
    filter: &str,
) -> Vec<NodeProcessInfo> {
    // First, detect all Node.js processes from the system
    let mut node_procs = detect_node_processes(system);

    // Try to load PM2 process list (gracefully handle errors)
    let pm2_procs = load_pm2_processes().unwrap_or_default();

    // Build a PID -> PM2 info map for quick lookup
    let pm2_by_pid: HashMap<Pid, &Pm2Process> = pm2_procs
        .iter()
        .filter_map(|p| p.pid.map(|pid| (Pid::from_u32(pid), p)))
        .collect();

    // Enrich Node processes with PM2 data
    for node_proc in &mut node_procs {
        if let Some(pm2_proc) = pm2_by_pid.get(&node_proc.pid) {
            node_proc.name = pm2_proc.name.clone();
            node_proc.pm2 = Some(Pm2Info {
                pm_id: pm2_proc.pm_id,
                name: pm2_proc.name.clone(),
                mode: pm2_proc.mode.clone(),
                status: pm2_proc.status.clone(),
                restarts: pm2_proc.restarts,
                pm2_uptime: pm2_proc.uptime_ms.map(|ms| ms / 1000),
            });
            if let Some(uptime_ms) = pm2_proc.uptime_ms {
                node_proc.uptime_secs = Some(uptime_ms / 1000);
            }
            if let Some(cpu) = pm2_proc.cpu {
                node_proc.cpu = cpu;
            }
        }
    }

    // Add PM2 processes that might not be running (stopped/errored)
    // but are still in PM2's process list
    for pm2_proc in &pm2_procs {
        let is_tracked = pm2_proc.pid
            .map(|pid| node_procs.iter().any(|n| n.pid.as_u32() == pid))
            .unwrap_or(false);

        if !is_tracked {
            // This is a PM2 process that's not currently running
            let script = pm2_proc.script.clone().unwrap_or_else(|| "-".to_string());
            let project_name = detect::project_name_from_script(&script);
            node_procs.push(NodeProcessInfo {
                pid: Pid::from_u32(pm2_proc.pid.unwrap_or(0)),
                name: pm2_proc.name.clone(),
                script,
                project_name,
                uses_nvm: false,
                node_version: None,
                cpu: pm2_proc.cpu.unwrap_or(0.0),
                memory_bytes: pm2_proc.memory_bytes.unwrap_or(0),
                uptime_secs: pm2_proc.uptime_ms.map(|ms| ms / 1000),
                pm2: Some(Pm2Info {
                    pm_id: pm2_proc.pm_id,
                    name: pm2_proc.name.clone(),
                    mode: pm2_proc.mode.clone(),
                    status: pm2_proc.status.clone(),
                    restarts: pm2_proc.restarts,
                    pm2_uptime: pm2_proc.uptime_ms.map(|ms| ms / 1000),
                }),
                worker_count: 1,
            });
        }
    }

    // Group cluster workers by script path (for non-PM2 processes)
    // PM2 processes are already unique per pm_id
    node_procs = group_cluster_workers(node_procs);

    // Apply filter if provided
    if !filter.is_empty() {
        let filter_lower = filter.to_lowercase();
        node_procs.retain(|p| {
            p.name.to_lowercase().contains(&filter_lower)
                || p.script.to_lowercase().contains(&filter_lower)
                || p.pid.to_string().contains(&filter_lower)
                || p.project_name
                    .as_deref()
                    .map_or(false, |name| name.to_lowercase().contains(&filter_lower))
                || p.pm2.as_ref().map_or(false, |pm2| {
                    pm2.status.to_lowercase().contains(&filter_lower)
                        || pm2.mode.to_lowercase().contains(&filter_lower)
                })
        });
    }

    // Sort by PM2 ID if available, then by PID
    node_procs.sort_by(|a, b| {
        match (&a.pm2, &b.pm2) {
            (Some(a_pm2), Some(b_pm2)) => a_pm2.pm_id.cmp(&b_pm2.pm_id),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.pid.cmp(&b.pid),
        }
    });

    node_procs
}

pub fn is_node_util(proc: &NodeProcessInfo) -> bool {
    proc.uses_nvm
        || contains_token(&proc.name, "nvm")
        || contains_token(&proc.script, "nvm")
        || proc
            .project_name
            .as_deref()
            .map_or(false, |name| contains_token(name, "nvm"))
        || contains_cli(&proc.name)
        || contains_cli(&proc.script)
        || proc
            .project_name
            .as_deref()
            .map_or(false, |name| contains_cli(name))
}

/// Group cluster workers running the same script into a single entry.
/// Aggregates CPU and memory, keeps the lowest PID as representative.
fn group_cluster_workers(procs: Vec<NodeProcessInfo>) -> Vec<NodeProcessInfo> {
    // PM2 processes have their own grouping, skip them
    let (pm2_procs, non_pm2_procs): (Vec<_>, Vec<_>) =
        procs.into_iter().partition(|p| p.pm2.is_some());

    // Group non-PM2 processes by script path
    let mut groups: HashMap<String, Vec<NodeProcessInfo>> = HashMap::new();
    for proc in non_pm2_procs {
        let key = proc.script.clone();
        groups.entry(key).or_default().push(proc);
    }

    // Merge each group into a single entry
    let mut merged: Vec<NodeProcessInfo> = Vec::new();
    for (_script, mut group) in groups {
        if group.len() == 1 {
            merged.push(group.remove(0));
        } else {
            // Sort by PID to get consistent "primary" process
            group.sort_by_key(|p| p.pid);

            // Aggregate stats
            let total_cpu: f32 = group.iter().map(|p| p.cpu).sum();
            let total_memory: u64 = group.iter().map(|p| p.memory_bytes).sum();
            let worker_count = group.len();
            let uses_nvm = group.iter().any(|p| p.uses_nvm);

            // Use first (lowest PID) as the primary
            let mut primary = group.remove(0);
            primary.cpu = total_cpu;
            primary.memory_bytes = total_memory;
            primary.worker_count = worker_count;
            primary.uses_nvm = uses_nvm;

            merged.push(primary);
        }
    }

    // Combine PM2 and merged non-PM2 processes
    merged.extend(pm2_procs);
    merged
}

pub fn group_node_processes(processes: &[NodeProcessInfo], offset: usize) -> Vec<NodeRow> {
    if processes.is_empty() {
        return Vec::new();
    }
    let indices: Vec<usize> = (0..processes.len()).collect();
    build_grouped_rows(processes, &indices, offset)
}

fn build_grouped_rows(
    processes: &[NodeProcessInfo],
    indices: &[usize],
    offset: usize,
) -> Vec<NodeRow> {
    if indices.is_empty() {
        return Vec::new();
    }

    let mut labels = Vec::with_capacity(indices.len());
    let mut tokens = Vec::with_capacity(indices.len());
    let mut token_keys = Vec::with_capacity(indices.len());
    let mut token_counts: HashMap<String, usize> = HashMap::new();

    for &idx in indices {
        let proc = &processes[idx];
        let label = group_label_for_node(proc);
        let token = group_token_from_label(&label);
        let token_key = token.to_ascii_lowercase();
        if !token_key.is_empty() {
            *token_counts.entry(token_key.clone()).or_insert(0) += 1;
        }
        labels.push(label);
        tokens.push(token);
        token_keys.push(token_key);
    }

    let mut groups: Vec<NodeGroup> = Vec::new();
    let mut group_map: HashMap<String, usize> = HashMap::new();

    for (i, &original_idx) in indices.iter().enumerate() {
        let use_token = token_counts
            .get(&token_keys[i])
            .copied()
            .unwrap_or(0)
            > 1;
        let (group_key, group_label) = if use_token && !token_keys[i].is_empty() {
            (
                format!("token::{}", token_keys[i]),
                tokens[i].clone(),
            )
        } else {
            (
                format!("label::{}", labels[i].to_ascii_lowercase()),
                labels[i].clone(),
            )
        };
        let group_index = match group_map.get(&group_key).copied() {
            Some(index) => index,
            None => {
                let index = groups.len();
                groups.push(NodeGroup {
                    name: group_label,
                    items: Vec::new(),
                });
                group_map.insert(group_key.clone(), index);
                index
            }
        };
        groups[group_index].items.push(original_idx + offset);
    }

    let mut rows = Vec::with_capacity(indices.len() + groups.len());
    for group in groups {
        rows.push(NodeRow::Group {
            name: group.name,
            count: group.items.len(),
        });
        for index in group.items {
            rows.push(NodeRow::Item { index });
        }
    }
    rows
}

struct NodeGroup {
    name: String,
    items: Vec<usize>,
}

fn group_label_for_node(proc: &NodeProcessInfo) -> String {
    if let Some(name) = proc.project_name.as_ref() {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    proc.name.trim().to_string()
}

fn group_token_from_label(label: &str) -> String {
    let trimmed = label.trim();
    let mut end = trimmed.len();
    for (idx, ch) in trimmed.char_indices() {
        if ch.is_whitespace() || matches!(ch, '|' | ':' | '-' | '_') {
            end = idx;
            break;
        }
    }
    let token = trimmed[..end].trim();
    if token.is_empty() {
        trimmed.to_string()
    } else {
        token.to_string()
    }
}

fn contains_cli(text: &str) -> bool {
    contains_token(text, "cli")
}

fn contains_token(text: &str, token: &str) -> bool {
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            if current == token {
                return true;
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        return current == token;
    }
    false
}

/// Format uptime as human-readable string.
pub fn format_uptime(secs: Option<u64>) -> String {
    match secs {
        None => "-".to_string(),
        Some(0) => "-".to_string(),
        Some(s) => {
            let days = s / 86400;
            let hours = (s % 86400) / 3600;
            let mins = (s % 3600) / 60;

            if days > 0 {
                format!("{}d {}h", days, hours)
            } else if hours > 0 {
                format!("{}h {}m", hours, mins)
            } else if mins > 0 {
                format!("{}m", mins)
            } else {
                format!("{}s", s)
            }
        }
    }
}
