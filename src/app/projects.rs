//! Resource relationships derived from snapshots, with cached background discovery.
use super::workspace_config::ProjectConfig;
use crate::system::{
    docker::{ContainerInfo, DockerListItem},
    node::NodeSnapshot,
    ports::PortInfo,
    process::{self, ProcessEntry},
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    Project(String),
    Process {
        pid: u32,
        identity: u64,
    },
    Container(String),
    Pm2 {
        id: u32,
        pid: Option<u32>,
    },
    Port {
        protocol: String,
        port: u16,
        pid: u32,
        container: Option<String>,
    },
    Volume(String),
}
impl Resource {
    pub fn key(&self) -> String {
        match self {
            Self::Project(key) => key.clone(),
            Self::Process { pid, identity } => format!("process:{pid}:{identity}"),
            Self::Container(id) => format!("container:{id}"),
            Self::Pm2 { id, .. } => format!("pm2:{id}"),
            Self::Port {
                protocol,
                port,
                pid,
                container,
            } => format!(
                "port:{protocol}:{port}:{pid}:{}",
                container.as_deref().unwrap_or("")
            ),
            Self::Volume(name) => format!("volume:{name}"),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Project(_) => "Project",
            Self::Process { .. } => "Process",
            Self::Container(_) => "Docker",
            Self::Pm2 { .. } => "PM2",
            Self::Port { .. } => "Port",
            Self::Volume(_) => "Volume",
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResourceRecord {
    pub resource: Resource,
    pub name: String,
    pub project: Option<String>,
    pub path: Option<String>,
    pub command: Option<String>,
    pub details: Vec<String>,
    pub cpu: Option<f32>,
    pub memory: Option<u64>,
    pub estimated: bool,
}
impl ResourceRecord {
    pub fn simple(resource: Resource, name: String) -> Self {
        Self {
            resource,
            name,
            project: None,
            path: None,
            command: None,
            details: Vec::new(),
            cpu: None,
            memory: None,
            estimated: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Project {
    pub key: String,
    pub name: String,
    pub path: Option<String>,
    pub resources: Vec<ResourceRecord>,
    pub cpu: f32,
    pub memory: u64,
    pub estimated: bool,
    pub running_containers: usize,
    pub unmeasured: usize,
}
impl Project {
    pub fn record(&self) -> ResourceRecord {
        let mut record =
            ResourceRecord::simple(Resource::Project(self.key.clone()), self.name.clone());
        record.project = Some(self.key.clone());
        record.path = self.path.clone();
        record.details = vec![
            format!(
                "{} resources · {} running containers",
                self.resources.len(),
                self.running_containers
            ),
            "RAM/CPU totals cover native processes and PM2. Container memory is shown separately in its inspector.".into(),
        ];
        record.cpu = (self.unmeasured == 0).then_some(self.cpu);
        record.memory = (self.unmeasured == 0).then_some(self.memory);
        if self.unmeasured > 0 {
            record.details.push(format!(
                "{} native/PM2 resources have missing measurements",
                self.unmeasured
            ));
        }
        record.estimated = self.estimated;
        record
    }
}
#[derive(Default)]
pub struct Discovery {
    processes: HashMap<(u32, u64), Option<(String, String)>>,
    roots: HashMap<PathBuf, Option<PathBuf>>,
}

pub fn project_key(path: &str) -> String {
    format!(
        "path:{}",
        Path::new(path).components().collect::<PathBuf>().display()
    )
}
fn name_for(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.into())
}
fn insert_project(
    projects: &mut BTreeMap<String, Project>,
    name: &str,
    path: Option<&str>,
) -> String {
    let key = path
        .map(project_key)
        .unwrap_or_else(|| format!("docker:{name}"));
    projects.entry(key.clone()).or_insert_with(|| Project {
        key: key.clone(),
        name: name.into(),
        path: path.map(str::to_string),
        resources: Vec::new(),
        cpu: 0.0,
        memory: 0,
        estimated: false,
        running_containers: 0,
        unmeasured: 0,
    });
    key
}
fn add(
    projects: &mut BTreeMap<String, Project>,
    key: &str,
    mut record: ResourceRecord,
    count_metrics: bool,
) {
    record.project = Some(key.into());
    if let Some(project) = projects.get_mut(key) {
        if count_metrics {
            if record.cpu.is_none() || record.memory.is_none() {
                project.unmeasured += 1;
            }
            project.cpu += record.cpu.unwrap_or(0.0);
            project.memory = project.memory.saturating_add(record.memory.unwrap_or(0));
            project.estimated |= record.estimated;
        }
        project.resources.push(record);
    }
}
fn matching_path<'a>(projects: &'a BTreeMap<String, Project>, cwd: &str) -> Option<&'a Project> {
    projects
        .values()
        .filter(|p| {
            p.path
                .as_deref()
                .is_some_and(|path| Path::new(cwd).starts_with(path))
        })
        .max_by_key(|p| p.path.as_ref().map_or(0, |s| s.len()))
}
impl Discovery {
    fn locate(
        &mut self,
        entry: &ProcessEntry,
        projects: &BTreeMap<String, Project>,
    ) -> Option<(String, String)> {
        let id = (entry.pid.as_u32(), entry.start_time);
        if let Some(cached) = self.processes.get(&id) {
            return cached.clone();
        }
        let result = (|| {
            if process::process_identity(id.0).ok()? != id.1 {
                return None;
            }
            let cwd = std::fs::read_link(format!("/proc/{}/cwd", id.0)).ok()?;
            let cwd = cwd.to_string_lossy().into_owned();
            if process::process_identity(id.0).ok()? != id.1 {
                return None;
            }
            if let Some(project) = matching_path(projects, &cwd) {
                return Some((project.name.clone(), project.path.clone()?));
            }
            let root = self
                .roots
                .entry(PathBuf::from(&cwd))
                .or_insert_with(|| {
                    Path::new(&cwd)
                        .ancestors()
                        .take(8)
                        .find(|dir| {
                            [
                                ".git",
                                "package.json",
                                "Cargo.toml",
                                "pyproject.toml",
                                "go.mod",
                                "compose.yaml",
                                "docker-compose.yml",
                            ]
                            .iter()
                            .any(|marker| dir.join(marker).exists())
                        })
                        .map(Path::to_path_buf)
                })
                .clone()?;
            let path = root.to_string_lossy().into_owned();
            Some((name_for(&path), path))
        })();
        self.processes.insert(id, result.clone());
        result
    }
}

pub fn build(
    discovery: &mut Discovery,
    processes: &[ProcessEntry],
    containers: &[ContainerInfo],
    node: &NodeSnapshot,
    ports: &[PortInfo],
    volumes: &[DockerListItem],
    configured: &[ProjectConfig],
) -> Vec<Project> {
    let mut projects = BTreeMap::new();
    for config in configured {
        insert_project(&mut projects, &config.name, Some(&config.path));
    }
    let mut container_projects = HashMap::new();
    for container in containers {
        let key = insert_project(
            &mut projects,
            &container.group_name,
            container.group_path.as_deref(),
        );
        container_projects.insert(container.id.clone(), key.clone());
        let mut record = ResourceRecord::simple(
            Resource::Container(container.id.clone()),
            container.name.clone(),
        );
        record.path = container.group_path.clone();
        record.command = Some(format!("docker logs --follow {}", container.id));
        record.details = vec![
            format!("ID: {}", container.id),
            format!("Image: {}", container.image),
            format!("Status: {}", container.status),
            format!("Published ports: {}", container.port_public),
        ];
        if container.running {
            projects.get_mut(&key).unwrap().running_containers += 1;
        }
        add(&mut projects, &key, record, false);
    }
    for pm2 in &node.pm2_procs {
        if let Some(cwd) = &pm2.cwd {
            let name = name_for(cwd);
            insert_project(&mut projects, &name, Some(cwd));
        }
    }
    let live: HashSet<_> = processes
        .iter()
        .map(|p| (p.pid.as_u32(), p.start_time))
        .collect();
    discovery.processes.retain(|id, _| live.contains(id));
    if discovery.roots.len() > 2048 {
        discovery.roots.clear();
    }
    let mut process_projects = HashMap::new();
    let mut counted = HashSet::new();
    for process in processes.iter().filter(|p| !p.is_thread) {
        if let Some((name, path)) = discovery.locate(process, &projects) {
            let key = matching_path(&projects, &path)
                .map(|p| p.key.clone())
                .unwrap_or_else(|| insert_project(&mut projects, &name, Some(&path)));
            let pid = process.pid.as_u32();
            counted.insert(pid);
            process_projects.insert(pid, key.clone());
            let mut record = ResourceRecord::simple(
                Resource::Process {
                    pid,
                    identity: process.start_time,
                },
                process.name.clone(),
            );
            record.path = Some(path);
            record.cpu = Some(process.cpu);
            record.memory = Some(
                process
                    .memory_sample
                    .map_or(process.memory_bytes, |m| m.pss_bytes),
            );
            record.estimated = process.memory_sample.is_none();
            record.details = vec![
                format!("PID: {pid}"),
                format!("Start identity: {}", process.start_time),
            ];
            add(&mut projects, &key, record, true);
        }
    }
    for proc in &node.node_procs {
        let pid = proc.pid.as_u32();
        if counted.contains(&pid) {
            if let Some(key) = process_projects.get(&pid) {
                if let Some(record) = projects.get_mut(key).and_then(|p| {
                    p.resources
                        .iter_mut()
                        .find(|r| matches!(r.resource,Resource::Process{pid:p,..} if p==pid))
                }) {
                    record.details.push(format!("Node script: {}", proc.script));
                    record.command = Some(format!("node {}", proc.script));
                }
            }
            continue;
        }
        // Native Node metadata alone cannot prove PID identity; omit an unsafe owner.
    }
    for pm2 in &node.pm2_procs {
        let key = pm2
            .pid
            .and_then(|pid| process_projects.get(&pid).cloned())
            .or_else(|| {
                pm2.cwd
                    .as_ref()
                    .map(|cwd| insert_project(&mut projects, &name_for(cwd), Some(cwd)))
            });
        if let Some(key) = key {
            let mut record = ResourceRecord::simple(
                Resource::Pm2 {
                    id: pm2.pm_id,
                    pid: pm2.pid,
                },
                pm2.name.clone(),
            );
            record.path = pm2.cwd.clone();
            record.command = pm2.script.clone();
            record.cpu = pm2.cpu;
            record.memory = pm2.memory_bytes;
            record.estimated = true;
            record.details = vec![
                format!(
                    "PM2 ID: {} · PID: {}",
                    pm2.pm_id,
                    pm2.pid.map_or("-".into(), |p| p.to_string())
                ),
                format!("Status: {} · Mode: {}", pm2.status, pm2.mode),
            ];
            let count = !pm2.pid.is_some_and(|pid| counted.contains(&pid));
            add(&mut projects, &key, record, count);
        }
    }
    for port in ports {
        let key = port
            .container_id
            .as_ref()
            .and_then(|id| exact_or_unique_prefix(&container_projects, id))
            .or_else(|| process_projects.get(&port.pid.as_u32()).cloned());
        if let Some(key) = key {
            let mut record = ResourceRecord::simple(
                Resource::Port {
                    protocol: port.proto.clone(),
                    port: port.port,
                    pid: port.pid.as_u32(),
                    container: port.container_id.clone(),
                },
                format!("{} {} · {}", port.proto, port.binding_display(), port.name),
            );
            record.command = Some(port.exe_path.clone());
            record.details = vec![
                format!("Owner: {} · PID: {}", port.name, port.pid),
                format!("Container: {}", port.container_id.as_deref().unwrap_or("-")),
            ];
            add(&mut projects, &key, record, false);
        }
    }
    for volume in volumes {
        let mut keys = HashSet::new();
        if let Some(attachments) = &volume.attachments {
            for a in attachments {
                if let Some(key) = exact_or_unique_prefix(&container_projects, &a.id) {
                    keys.insert(key);
                } else if let Some(dir) = &a.project_dir {
                    keys.insert(insert_project(
                        &mut projects,
                        a.project.as_deref().unwrap_or(&name_for(dir)),
                        Some(dir),
                    ));
                } else if let Some(name) = &a.project {
                    keys.insert(insert_project(&mut projects, name, None));
                }
            }
        }
        if keys.is_empty() {
            keys.insert(insert_project(&mut projects, "Unassigned volumes", None));
        }
        for key in keys {
            let mut record =
                ResourceRecord::simple(Resource::Volume(volume.name.clone()), volume.name.clone());
            record.details = vec![
                format!("Size: {}", volume.size),
                format!(
                    "Activity: {}",
                    volume.activity.as_deref().unwrap_or("Unknown")
                ),
                format!("Containers: {}", volume.detail_left),
                format!("Images: {}", volume.detail_right),
            ];
            add(&mut projects, &key, record, false);
        }
    }
    for project in projects.values_mut() {
        project.resources.sort_by(|a, b| {
            a.resource
                .kind()
                .cmp(b.resource.kind())
                .then(a.name.cmp(&b.name))
        });
    }
    projects.into_values().collect()
}
pub fn exact_or_unique_prefix(map: &HashMap<String, String>, id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    if let Some(value) = map.get(id) {
        return Some(value.clone());
    }
    let mut matches = map.iter().filter(|(key, _)| key.starts_with(id));
    let value = matches.next()?.1.clone();
    if matches.next().is_some() {
        None
    } else {
        Some(value)
    }
}

pub fn process_details(
    pid: u32,
    identity: u64,
) -> std::io::Result<(Option<String>, Option<String>)> {
    if process::process_identity(pid)? != identity {
        return Err(std::io::Error::other("Process exited or PID was reused"));
    }
    let path = std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    use std::io::Read;
    let mut bytes = Vec::new();
    let readable = std::fs::File::open(format!("/proc/{pid}/cmdline"))
        .and_then(|file| file.take(65537).read_to_end(&mut bytes));
    if bytes.len() > 65536 {
        return Err(std::io::Error::other(
            "Process command exceeds 64 KiB; not displayed or copied",
        ));
    }
    let command = readable.map(|_| bytes).ok().map(|bytes| {
        String::from_utf8_lossy(&bytes)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    });
    if process::process_identity(pid)? != identity {
        return Err(std::io::Error::other(
            "Process identity changed while inspecting",
        ));
    }
    Ok((path, command))
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_/-.:=@+".contains(&ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace("'", "'\"'\"'"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_pm2_measurements_are_not_presented_as_complete_zero_totals() {
        let mut node = NodeSnapshot::default();
        node.pm2_procs.push(crate::system::node::Pm2Process {
            pm_id: 1,
            name: "pending".into(),
            pid: None,
            mode: "fork".into(),
            status: "stopped".into(),
            cpu: None,
            memory_bytes: None,
            uptime_ms: None,
            script: None,
            cwd: Some("/srv/demo".into()),
        });
        let projects = build(&mut Discovery::default(), &[], &[], &node, &[], &[], &[]);
        assert_eq!(projects[0].unmeasured, 1);
        assert_eq!(projects[0].record().memory, None);
        assert_eq!(projects[0].record().cpu, None);
    }
    #[test]
    fn projects_join_native_pm2_ports_and_volume_owners_without_double_counting() {
        let mut discovery = Discovery::default();
        discovery
            .processes
            .insert((100, 1), Some(("demo".into(), "/srv/demo".into())));
        let processes = vec![ProcessEntry {
            pid: sysinfo::Pid::from_u32(100),
            name: "node".into(),
            cpu: 5.0,
            memory_bytes: 900,
            start_time: 1,
            memory_sample: Some(process::MemorySample {
                pss_bytes: 200,
                swap_pss_bytes: None,
            }),
            user_id: None,
            parent: None,
            is_thread: false,
        }];
        let containers = vec![ContainerInfo {
            id: "abc123456789abcd".into(),
            name: "api".into(),
            image: "demo:v1".into(),
            port_public: "8080".into(),
            port_internal: "80".into(),
            status: "Up".into(),
            group_name: "demo".into(),
            group_path: Some("/srv/demo".into()),
            running: true,
            memory: None,
            activity_secs: 0,
        }];
        let mut node = NodeSnapshot::default();
        node.pm2_procs.push(crate::system::node::Pm2Process {
            pm_id: 7,
            name: "worker".into(),
            pid: Some(100),
            mode: "fork".into(),
            status: "online".into(),
            cpu: Some(5.0),
            memory_bytes: Some(900),
            uptime_ms: None,
            script: Some("/srv/demo/main.js".into()),
            cwd: Some("/srv/demo".into()),
        });
        let ports = vec![PortInfo {
            proto: "tcp".into(),
            port: 8080,
            internal_port: Some(80),
            pid: sysinfo::Pid::from_u32(0),
            name: "api".into(),
            exe_path: "-".into(),
            container_id: Some("abc123456789".into()),
            group_name: None,
            project_name: None,
        }];
        let volumes = vec![DockerListItem {
            name: "data".into(),
            size: "950 GB".into(),
            attachments: Some(vec![crate::system::docker::VolumeAttachment {
                id: containers[0].id.clone(),
                container: "api".into(),
                project: Some("demo".into()),
                project_dir: Some("/srv/demo".into()),
                ..Default::default()
            }]),
            ..Default::default()
        }];
        let projects = build(
            &mut discovery,
            &processes,
            &containers,
            &node,
            &ports,
            &volumes,
            &[],
        );
        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.resources.len(), 5);
        assert_eq!(project.memory, 200);
        assert_eq!(project.cpu, 5.0);
        assert!(!project.estimated);
        assert_eq!(project.running_containers, 1);
        assert!(project
            .resources
            .iter()
            .all(|r| r.project.as_deref() == Some("path:/srv/demo")));
    }
    #[test]
    fn ambiguous_ids_unknown_volumes_and_stopped_configuration_stay_explicit() {
        let map = HashMap::from([
            ("abc123".into(), "one".into()),
            ("abc456".into(), "two".into()),
        ]);
        assert_eq!(exact_or_unique_prefix(&map, "abc"), None);
        assert_eq!(exact_or_unique_prefix(&map, "abc123"), Some("one".into()));
        let projects = build(
            &mut Discovery::default(),
            &[],
            &[],
            &NodeSnapshot::default(),
            &[],
            &[DockerListItem {
                name: "orphan".into(),
                size: "Unknown".into(),
                attachments: None,
                ..Default::default()
            }],
            &[ProjectConfig {
                name: "stopped".into(),
                path: "/srv/stopped".into(),
                ..Default::default()
            }],
        );
        assert_eq!(projects.len(), 2);
        assert!(projects
            .iter()
            .any(|p| p.name == "stopped" && p.resources.is_empty()));
        assert!(projects.iter().any(|p| p.name == "Unassigned volumes"));
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
