mod container;
mod stats;
mod terminal;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub use container::{container_label_for, kill_container, load_container_env, load_docker_container_cache};
pub use stats::{apply_container_filter, group_containers, load_docker_stats};
pub use terminal::{open_container_logs, open_container_shell};

#[derive(Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub port_public: String,
    pub port_internal: String,
    pub status: String,
    pub cpu: f32,
    pub memory_bytes: u64,
    pub group_name: String,
    pub group_path: Option<String>,
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
