mod container;
mod stats;
mod terminal;

use std::borrow::Cow;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::util::{contains_lower, Filterable};

pub use container::{
    kill_container, kill_containers, load_container_env, prune_build_cache, prune_dangling_images,
    prune_volumes, restart_container, load_container_logs,
    start_container, stop_container,
};
pub use stats::{
    apply_container_filter, group_containers, load_docker_stats, load_docker_system_df,
    DockerSystemDf,
};
pub use terminal::open_container_shell;

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
    pub group_name: Cow<'static, str>,
    pub group_path: Option<String>,
    pub running: bool,
    /// Seconds since last activity (lower = more recent)
    pub activity_secs: u64,
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
        running_count: usize,
    },
    Item { index: usize, prefix: String },
    Separator,
}

pub struct DockerStatsWorker {
    /// Uses RwLock for reader-priority access - main thread reads frequently,
    /// worker thread writes infrequently
    /// Inner Arc allows snapshot() to return without cloning the vector data
    data: Arc<RwLock<Arc<Vec<ContainerInfo>>>>,
    paused: Arc<AtomicBool>,
}

impl DockerStatsWorker {
    /// Returns an Arc-wrapped snapshot of container data.
    /// This is a cheap pointer clone, not a full data clone.
    pub fn snapshot(&self) -> Arc<Vec<ContainerInfo>> {
        // Use read lock - allows multiple concurrent readers
        let guard = self.data.read().unwrap_or_else(|err| err.into_inner());
        Arc::clone(&guard)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

pub fn start_docker_stats_worker(interval: Duration) -> DockerStatsWorker {
    let data = Arc::new(RwLock::new(Arc::new(Vec::new())));
    let thread_data = Arc::clone(&data);
    let paused = Arc::new(AtomicBool::new(false));
    let thread_paused = Arc::clone(&paused);

    thread::spawn(move || loop {
        if thread_paused.load(Ordering::Relaxed) {
            thread::sleep(interval);
            continue;
        }
        if let Some(stats) = load_docker_stats() {
            // Use write lock - only held briefly while replacing the Arc pointer
            let mut guard = thread_data.write().unwrap_or_else(|err| err.into_inner());
            *guard = Arc::new(stats);
        }
        thread::sleep(interval);
    });

    DockerStatsWorker { data, paused }
}

/// Background worker for docker system df data
pub struct DockerSystemDfWorker {
    data: Arc<RwLock<DockerSystemDf>>,
    paused: Arc<AtomicBool>,
}

impl DockerSystemDfWorker {
    /// Returns the current docker system df data
    pub fn snapshot(&self) -> DockerSystemDf {
        let guard = self.data.read().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

pub fn start_docker_df_worker(interval: Duration) -> DockerSystemDfWorker {
    let data = Arc::new(RwLock::new(DockerSystemDf::default()));
    let thread_data = Arc::clone(&data);
    let paused = Arc::new(AtomicBool::new(false));
    let thread_paused = Arc::clone(&paused);

    thread::spawn(move || loop {
        if thread_paused.load(Ordering::Relaxed) {
            thread::sleep(interval);
            continue;
        }
        if let Some(df) = load_docker_system_df() {
            let mut guard = thread_data.write().unwrap_or_else(|err| err.into_inner());
            *guard = df;
        }
        thread::sleep(interval);
    });

    DockerSystemDfWorker { data, paused }
}
