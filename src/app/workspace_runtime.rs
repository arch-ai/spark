//! Snapshot-driven project assembly, coalesced on a single background task.
use super::{
    projects::{self, Discovery, Project},
    sorting::SortTarget,
    AppState,
};
use crate::system::{
    docker::{ContainerInfo, DockerListItem},
    node::NodeSnapshot,
    ports::PortInfo,
    process::ProcessEntry,
};
use std::sync::{mpsc, Arc};
use std::time::SystemTime;

struct Built {
    projects: Vec<Project>,
    discovery: Discovery,
    at: SystemTime,
    sample: bool,
    processes: Arc<Vec<ProcessEntry>>,
}
pub struct Catalog {
    processes: Arc<Vec<ProcessEntry>>,
    containers: Arc<Vec<ContainerInfo>>,
    node: Arc<NodeSnapshot>,
    ports: Arc<Vec<PortInfo>>,
    volumes: Arc<Vec<DockerListItem>>,
    revision: u64,
    dirty: bool,
    busy: bool,
    sample: bool,
    discovery: Discovery,
    reset_discovery: bool,
    rx: mpsc::Receiver<Built>,
    tx: mpsc::Sender<Built>,
}
impl Default for Catalog {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            processes: Arc::new(Vec::new()),
            containers: Arc::new(Vec::new()),
            node: Arc::new(NodeSnapshot::default()),
            ports: Arc::new(Vec::new()),
            volumes: Arc::new(Vec::new()),
            revision: u64::MAX,
            dirty: true,
            busy: false,
            sample: false,
            discovery: Discovery::default(),
            reset_discovery: false,
            rx,
            tx,
        }
    }
}
impl Catalog {
    pub fn update(
        &mut self,
        state: &mut AppState,
        processes: Arc<Vec<ProcessEntry>>,
        containers: Arc<Vec<ContainerInfo>>,
        node: Arc<NodeSnapshot>,
        ports: Arc<Vec<PortInfo>>,
    ) -> bool {
        let mut changed = false;
        if !Arc::ptr_eq(&self.processes, &processes) {
            self.dirty = true;
            self.sample = true;
            self.processes = processes;
        }
        if !Arc::ptr_eq(&self.containers, &containers) {
            self.dirty = true;
            self.containers = containers;
        }
        if !Arc::ptr_eq(&self.node, &node) {
            self.dirty = true;
            if self.processes.is_empty() {
                self.sample = true;
            }
            self.node = node;
        }
        if !Arc::ptr_eq(&self.ports, &ports) {
            self.dirty = true;
            self.ports = ports;
        }
        if !Arc::ptr_eq(&self.volumes, &state.workspace.volumes) {
            self.dirty = true;
            self.volumes = state.workspace.volumes.clone();
        }
        if self.revision != state.workspace.catalog_version {
            self.dirty = true;
            self.revision = state.workspace.catalog_version;
            self.reset_discovery = true;
        }
        while let Ok(built) = self.rx.try_recv() {
            let selected = state
                .workspace
                .visible_projects(state.sort_for(SortTarget::Projects))
                .get(state.workspace.selected)
                .map(|p| p.key.clone());
            self.discovery = built.discovery;
            self.busy = false;
            if built.sample {
                state.workspace.history.observe(&built.projects, built.at);
                state.workspace.history.native_snapshot(
                    &built.processes,
                    &built.projects,
                    built.at,
                );
            }
            state.workspace.replace_projects(built.projects);
            if let Some(key) = selected {
                if let Some(index) = state
                    .workspace
                    .visible_projects(state.sort_for(SortTarget::Projects))
                    .iter()
                    .position(|p| p.key == key)
                {
                    state.workspace.selected = index;
                }
            }
            let count = state
                .workspace
                .visible_projects(state.sort_for(SortTarget::Projects))
                .len();
            state.workspace.selected = state.workspace.selected.min(count.saturating_sub(1));
            changed = true;
        }
        if self.dirty && !self.busy {
            self.dirty = false;
            self.busy = true;
            let sample = std::mem::take(&mut self.sample);
            let mut discovery = if std::mem::take(&mut self.reset_discovery) {
                Discovery::default()
            } else {
                std::mem::take(&mut self.discovery)
            };
            let processes = self.processes.clone();
            let containers = self.containers.clone();
            let node = self.node.clone();
            let ports = self.ports.clone();
            let volumes = self.volumes.clone();
            let configured = state.workspace.preferences.projects.clone();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let at = SystemTime::now();
                let projects = projects::build(
                    &mut discovery,
                    &processes,
                    &containers,
                    &node,
                    &ports,
                    &volumes,
                    &configured,
                );
                let _ = tx.send(Built {
                    projects,
                    discovery,
                    at,
                    sample,
                    processes,
                });
            });
        }
        state.workspace.catalog_loading = self.busy;
        changed
    }
}
