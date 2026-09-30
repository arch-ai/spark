pub(crate) mod command;
mod container;
mod stats;
mod terminal;
mod worker;
mod volumes;

use std::borrow::Cow;
use std::time::Duration;

use crate::util::{contains_lower, Filterable};

pub use container::{
    delete_docker_container, delete_docker_image, inspect_docker_container,
    inspect_docker_image, kill_container, load_container_env,
    load_container_logs, load_docker_containers_with_size, load_docker_images,
    prune_build_cache, prune_dangling_images, prune_volumes, restart_container, start_container,
    stop_container, DockerListItem,
};
pub use stats::{
    apply_container_filter, group_containers, load_docker_stats, load_docker_system_df,
    DockerSystemDf,
};
pub use terminal::{open_container_logs, open_container_shell};
pub use volumes::{delete_docker_volume, inspect_docker_volume, load_docker_volumes};

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

pub fn matches_group(container: &ContainerInfo, name: &str, path: Option<&str>) -> bool {
    container.group_path.as_deref() == path && (path.is_some() || container.group_name == name)
}

pub fn start_docker_stats_worker(interval: Duration) -> worker::DockerWorker<Vec<ContainerInfo>> {
    worker::start_worker(interval, load_docker_stats)
}

pub fn start_docker_df_worker(interval: Duration) -> worker::DockerWorker<DockerSystemDf> {
    worker::start_worker(interval, load_docker_system_df)
}
