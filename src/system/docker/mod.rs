mod container;
mod stats;
mod terminal;

use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::util::{contains_lower, Filterable};

pub use container::{container_label_for, kill_container, load_container_env, load_docker_container_cache};
pub use stats::{apply_container_filter, group_containers, load_docker_stats};
pub use terminal::{open_container_logs, open_container_shell};

/// Container information with optimized string storage.
/// Uses Cow<'static, str> for fields that often contain static values like "-".
#[derive(Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: Cow<'static, str>,
    pub port_public: Cow<'static, str>,
    pub port_internal: Cow<'static, str>,
    pub status: Cow<'static, str>,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub group_name: Cow<'static, str>,
    pub group_path: Option<String>,
}

impl Filterable for ContainerInfo {
    fn matches_filter(&self, filter_lower: &str) -> bool {
        contains_lower(&self.id, filter_lower)
            || contains_lower(&self.name, filter_lower)
            || contains_lower(&self.image, filter_lower)
            || contains_lower(&self.port_public, filter_lower)
            || contains_lower(&self.port_internal, filter_lower)
            || contains_lower(&self.status, filter_lower)
            || contains_lower(&self.group_name, filter_lower)
            || self.group_path.as_deref().map_or(false, |p| contains_lower(p, filter_lower))
    }
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
