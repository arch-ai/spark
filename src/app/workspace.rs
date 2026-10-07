//! Shared workspace state; live readers and expensive requests stay off the UI thread.
use super::{
    history::{History, ObservedEvent},
    launcher::{LaunchJob, LaunchMessage, Mode},
    projects::{self, Project, Resource, ResourceRecord},
    workspace_config::{Preferences, ProjectConfig},
};
use crate::system::{
    docker::{self, DockerListItem},
    live::{self, LiveStream, StreamMessage},
    storage::{self, CleanupPlan},
};
use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;

pub const LOG_LIMIT: usize = 2000;
type DetailRequest = mpsc::Receiver<io::Result<(Option<String>, Option<String>)>>;
const LOG_BYTES_LIMIT: usize = 2 * 1024 * 1024;
fn append_log(lines: &mut VecDeque<LogLine>, bytes: &mut usize, text: String, error: bool) {
    *bytes += text.len();
    lines.push_back(LogLine { text, error });
    while lines.len() > LOG_LIMIT || *bytes > LOG_BYTES_LIMIT {
        if let Some(old) = lines.pop_front() {
            *bytes = bytes.saturating_sub(old.text.len());
        } else {
            break;
        }
    }
}
#[derive(Clone)]
pub struct LogLine {
    pub text: String,
    pub error: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorTab {
    Details,
    Logs,
    Events,
    Trends,
    Storage,
    Run,
}
impl InspectorTab {
    pub const ALL: [Self; 6] = [
        Self::Details,
        Self::Logs,
        Self::Events,
        Self::Trends,
        Self::Storage,
        Self::Run,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::Logs => "Logs",
            Self::Events => "Events",
            Self::Trends => "Trends",
            Self::Storage => "Storage",
            Self::Run => "Run",
        }
    }
}
pub struct Inspector {
    pub record: ResourceRecord,
    pub tab: InspectorTab,
    pub scroll: usize,
    pub resource_selected: usize,
    pub logs: VecDeque<LogLine>,
    logs_bytes: usize,
    pub frozen: Option<Vec<LogLine>>,
    pub query: String,
    pub searching: bool,
    pub follow: bool,
    pub stream: Option<LiveStream>,
    pub stream_status: String,
    pub detail_request: Option<DetailRequest>,
    pub back: Option<ResourceRecord>,
}
impl Inspector {
    fn new(record: ResourceRecord) -> Self {
        Self {
            record,
            tab: InspectorTab::Details,
            scroll: 0,
            resource_selected: 0,
            logs: VecDeque::new(),
            logs_bytes: 0,
            frozen: None,
            query: String::new(),
            searching: false,
            follow: true,
            stream: None,
            stream_status: "Press l or select Logs to start streaming".into(),
            detail_request: None,
            back: None,
        }
    }
    pub fn push(&mut self, text: String, error: bool) {
        let lower = text.to_ascii_lowercase();
        let error = error
            || ["error", "fatal", "panic", "exception", "failed"]
                .iter()
                .any(|word| lower.contains(word));
        append_log(&mut self.logs, &mut self.logs_bytes, text, error);
    }
    pub fn visible_logs(&self) -> Vec<&LogLine> {
        let query = self.query.to_lowercase();
        let lines: Box<dyn Iterator<Item = &LogLine> + '_> = match &self.frozen {
            Some(lines) => Box::new(lines.iter()),
            None => Box::new(self.logs.iter()),
        };
        lines
            .filter(|line| query.is_empty() || line.text.to_lowercase().contains(&query))
            .collect()
    }
}
pub enum Cleanup {
    Idle,
    Loading(mpsc::Receiver<io::Result<CleanupPlan>>),
    Review(CleanupPlan),
    Running(mpsc::Receiver<io::Result<String>>),
    Result(String, bool),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Project,
    SaveFilter,
    Filters,
}
pub struct Editor {
    pub kind: EditorKind,
    pub config: ProjectConfig,
    pub field: usize,
    pub text: String,
    pub error: Option<String>,
}
pub struct RunState {
    pub key: String,
    pub mode: Mode,
    pub job: Option<LaunchJob>,
    pub lines: VecDeque<LogLine>,
    lines_bytes: usize,
    pub result: Option<Result<String, String>>,
}
pub struct Workspace {
    pub projects: Vec<Project>,
    pub filter: String,
    pub selected: usize,
    pub scroll: usize,
    pub preferences: Preferences,
    pub config_path: Option<PathBuf>,
    pub config_error: Option<String>,
    pub inspector: Option<Inspector>,
    pub history: History,
    pub event_stream: Option<LiveStream>,
    pub events_started: bool,
    pub events_error: Option<String>,
    pub volumes: Arc<Vec<DockerListItem>>,
    pub storage_request: Option<mpsc::Receiver<io::Result<Vec<DockerListItem>>>>,
    pub storage_error: Option<String>,
    pub storage_loaded: bool,
    pub storage_selected: usize,
    pub selected_volumes: BTreeSet<String>,
    pub cleanup: Cleanup,
    pub cleanup_open: bool,
    pub review_cancelled: bool,
    pub review_scroll: usize,
    pub editor: Option<Editor>,
    pub runs: Vec<RunState>,
    pub clipboard: Option<String>,
    pub navigation: Option<Resource>,
    pub navigation_started: Option<std::time::Instant>,
    pub owner_request: Option<mpsc::Receiver<io::Result<Resource>>>,
    pub catalog_loading: bool,
    pub catalog_version: u64,
    pub catalog_error: Option<String>,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            filter: String::new(),
            selected: 0,
            scroll: 0,
            preferences: Preferences::default(),
            config_path: None,
            config_error: None,
            inspector: None,
            history: History::default(),
            event_stream: None,
            events_started: false,
            events_error: None,
            volumes: Arc::new(Vec::new()),
            storage_request: None,
            storage_error: None,
            storage_loaded: false,
            storage_selected: 0,
            selected_volumes: BTreeSet::new(),
            cleanup: Cleanup::Idle,
            cleanup_open: false,
            review_cancelled: false,
            review_scroll: 0,
            editor: None,
            runs: Vec::new(),
            clipboard: None,
            navigation: None,
            navigation_started: None,
            owner_request: None,
            catalog_loading: false,
            catalog_version: 0,
            catalog_error: None,
        }
    }
}

impl Workspace {
    pub fn replace_projects(&mut self, projects: Vec<Project>) {
        let selected = self
            .inspector
            .as_ref()
            .and_then(|i| match &i.record.resource {
                Resource::Project(key) => self
                    .project(key)
                    .and_then(|p| p.resources.get(i.resource_selected))
                    .map(|r| r.resource.key()),
                _ => None,
            });
        self.projects = projects;
        if let Some(inspector) = &mut self.inspector {
            if let Resource::Project(key) = &inspector.record.resource {
                if let Some(project) = self.projects.iter().find(|p| &p.key == key) {
                    inspector.resource_selected = selected
                        .as_ref()
                        .and_then(|key| {
                            project
                                .resources
                                .iter()
                                .position(|r| &r.resource.key() == key)
                        })
                        .unwrap_or(
                            inspector
                                .resource_selected
                                .min(project.resources.len().saturating_sub(1)),
                        );
                    inspector.record = project.record();
                }
            } else if let Some(fresh) = self
                .projects
                .iter()
                .flat_map(|p| p.resources.iter())
                .find(|r| r.resource.key() == inspector.record.resource.key())
            {
                let mut fresh = fresh.clone();
                if matches!(fresh.resource, Resource::Process { .. }) {
                    fresh.path = inspector.record.path.clone().or(fresh.path);
                    fresh.command = inspector.record.command.clone().or(fresh.command);
                }
                inspector.record = fresh;
            }
        }
    }
    pub fn project(&self, key: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.key == key)
    }
    pub fn visible_projects(&self, sort: super::sorting::TableSort) -> Vec<&Project> {
        let filter = self.filter.to_lowercase();
        let mut rows: Vec<_> = self
            .projects
            .iter()
            .filter(|p| {
                filter.is_empty()
                    || format!(
                        "{} {} {}",
                        p.name,
                        p.path.as_deref().unwrap_or(""),
                        p.resources
                            .iter()
                            .map(|r| r.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .to_lowercase()
                    .contains(&filter)
            })
            .collect();
        rows.sort_by(|a, b| {
            let favorites = self
                .preferences
                .favorites
                .contains(&b.key)
                .cmp(&self.preferences.favorites.contains(&a.key));
            let order = match sort.field {
                super::sorting::SortField::Cpu => a.cpu.total_cmp(&b.cpu),
                super::sorting::SortField::Memory => a.memory.cmp(&b.memory),
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            favorites
                .then(if sort.order == super::SortOrder::Desc {
                    order.reverse()
                } else {
                    order
                })
                .then(a.key.cmp(&b.key))
        });
        rows
    }
    pub fn inspect(&mut self, record: ResourceRecord) {
        let mut inspector = Inspector::new(record);
        if let Resource::Process { pid, identity } = inspector.record.resource {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(projects::process_details(pid, identity));
            });
            inspector.detail_request = Some(rx);
        }
        self.inspector = Some(inspector);
        self.storage_selected = 0;
    }
    pub fn open_logs(&mut self) {
        let Some(inspector) = &mut self.inspector else {
            return;
        };
        inspector.tab = InspectorTab::Logs;
        inspector.scroll = 0;
        if inspector.stream.is_some() {
            return;
        }
        let stream = match &inspector.record.resource {
            Resource::Container(id) => live::container_logs(id),
            Resource::Pm2 { id, .. } => live::pm2_logs(*id),
            Resource::Process { pid, identity } => live::process_logs(*pid, *identity),
            _ => {
                inspector.stream_status =
                    "Choose a process, PM2 app, or container in Details to read its logs".into();
                return;
            }
        };
        match stream {
            Ok(stream) => {
                inspector.stream = Some(stream);
                inspector.stream_status =
                    "Following live output · p pause · / search · End follow".into();
            }
            Err(error) => {
                inspector.stream_status =
                    format!("Could not start logs: {error}. Press l to retry.");
            }
        }
    }
    pub fn start_events(&mut self) {
        if self.events_started {
            return;
        }
        self.events_started = true;
        match live::docker_events() {
            Ok(stream) => {
                self.event_stream = Some(stream);
                self.events_error = None;
            }
            Err(error) => self.events_error = Some(format!("Docker events unavailable: {error}")),
        }
    }
    pub fn load_storage(&mut self) {
        if self.storage_request.is_some() || self.cleanup_busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.storage_request = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(docker::load_docker_volumes());
        });
    }
    pub fn storage_items(&self) -> Vec<&DockerListItem> {
        let key = self
            .inspector
            .as_ref()
            .and_then(|i| match &i.record.resource {
                Resource::Project(key) => Some(key),
                _ => i.record.project.as_ref(),
            });
        let names: Option<BTreeSet<&str>> = key.and_then(|key| self.project(key)).map(|p| {
            p.resources
                .iter()
                .filter_map(|r| match &r.resource {
                    Resource::Volume(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect()
        });
        let volume = self
            .inspector
            .as_ref()
            .and_then(|i| match &i.record.resource {
                Resource::Volume(name) => Some(name),
                _ => None,
            });
        let mut rows: Vec<_> = self
            .volumes
            .iter()
            .filter(|item| {
                volume.is_some_and(|name| name == &item.name)
                    || volume.is_none()
                        && names
                            .as_ref()
                            .is_none_or(|names| names.contains(item.name.as_str()))
            })
            .collect();
        rows.sort_by(|a, b| {
            super::sorting::size_bytes(&b.size)
                .cmp(&super::sorting::size_bytes(&a.size))
                .then(a.name.cmp(&b.name))
        });
        rows
    }
    pub fn review_cleanup(&mut self) {
        if self.cleanup_busy() || self.selected_volumes.is_empty() {
            return;
        }
        let visible: BTreeSet<_> = self
            .storage_items()
            .iter()
            .map(|item| item.name.clone())
            .collect();
        let names: Vec<_> = self
            .selected_volumes
            .intersection(&visible)
            .cloned()
            .collect();
        if names.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(storage::preview(&names));
        });
        self.cleanup = Cleanup::Loading(rx);
        self.cleanup_open = true;
        self.review_cancelled = false;
        self.review_scroll = 0;
    }
    pub fn execute_cleanup(&mut self) {
        let Cleanup::Review(plan) = std::mem::replace(&mut self.cleanup, Cleanup::Idle) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(storage::execute(&plan));
        });
        self.cleanup = Cleanup::Running(rx);
    }
    pub fn cleanup_busy(&self) -> bool {
        matches!(self.cleanup, Cleanup::Loading(_) | Cleanup::Running(_))
    }
    pub fn configure(&mut self, key: Option<&str>) {
        let project = key.and_then(|key| self.project(key));
        let config = project
            .and_then(|p| p.path.as_ref())
            .and_then(|path| self.preferences.projects.iter().find(|p| &p.path == path))
            .cloned()
            .unwrap_or_else(|| ProjectConfig {
                name: project.map_or(String::new(), |p| p.name.clone()),
                path: project.and_then(|p| p.path.clone()).unwrap_or_default(),
                ..ProjectConfig::default()
            });
        self.editor = Some(Editor {
            kind: EditorKind::Project,
            text: config.path.clone(),
            config,
            field: 0,
            error: None,
        });
    }
    pub fn launch(&mut self, key: &str, mode: Mode) -> Result<(), String> {
        if self
            .runs
            .iter()
            .any(|run| run.key == key && run.job.is_some())
        {
            return Err("A script is already running for this project".into());
        }
        let path = self
            .project(key)
            .and_then(|p| p.path.as_ref())
            .ok_or("This project has no local path; configure it first")?;
        let config = self
            .preferences
            .projects
            .iter()
            .find(|p| &p.path == path)
            .cloned()
            .ok_or("Configure project scripts with C first")?;
        self.runs.retain(|run| run.key != key);
        if self.runs.len() >= 16 {
            if let Some(index) = self.runs.iter().position(|run| run.job.is_none()) {
                self.runs.remove(index);
            } else {
                return Err("Too many running scripts".into());
            }
        }
        let job = LaunchJob::start(config, mode);
        self.runs.push(RunState {
            key: key.into(),
            mode,
            job: Some(job),
            lines: VecDeque::new(),
            lines_bytes: 0,
            result: None,
        });
        Ok(())
    }
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        if let Some(stream) = &self.event_stream {
            for message in stream.drain() {
                changed = true;
                match message {
                    StreamMessage::Line { text, stderr } => {
                        if stderr {
                            self.events_error = Some(text);
                        } else {
                            self.history.docker_event(&text, &self.projects);
                        }
                    }
                    StreamMessage::Exit(result) => {
                        self.events_error = Some(format!(
                            "Docker event stream ended: {}. Press F5 on Events to reconnect.",
                            match result {
                                Ok(status) => status.to_string(),
                                Err(e) => e.to_string(),
                            }
                        ));
                    }
                }
            }
        }
        if let Some(inspector) = &mut self.inspector {
            let messages = inspector
                .stream
                .as_ref()
                .map(|stream| stream.drain())
                .unwrap_or_default();
            for message in messages {
                changed = true;
                match message {
                    StreamMessage::Line { text, stderr } => inspector.push(text, stderr),
                    StreamMessage::Exit(result) => {
                        inspector.stream_status = format!(
                            "Stream ended: {} · l reconnect",
                            match result {
                                Ok(status) => status.to_string(),
                                Err(e) => e.to_string(),
                            }
                        );
                        inspector.stream = None;
                    }
                }
            }
            if let Some(result) = inspector
                .detail_request
                .as_ref()
                .and_then(|rx| rx.try_recv().ok())
            {
                inspector.detail_request = None;
                changed = true;
                match result {
                    Ok((path, command)) => {
                        inspector.record.path = path;
                        if inspector.record.name.starts_with("PID ") {
                            if let Some(name) = command
                                .as_deref()
                                .and_then(|cmd| cmd.split_whitespace().next())
                                .and_then(|exe| std::path::Path::new(exe).file_name())
                            {
                                inspector.record.name = name.to_string_lossy().into_owned();
                            }
                        }
                        inspector.record.command = command;
                    }
                    Err(e) => inspector.record.details.push(e.to_string()),
                }
            }
        }
        if let Some(result) = self
            .storage_request
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.storage_request = None;
            changed = true;
            match result {
                Ok(mut items) => {
                    super::sorting::sort_resources(
                        &mut items,
                        super::sorting::TableSort::new(
                            super::sorting::SortField::Size,
                            super::SortOrder::Desc,
                        ),
                    );
                    self.volumes = Arc::new(items);
                    self.storage_loaded = true;
                    self.storage_error = None;
                    self.selected_volumes
                        .retain(|name| self.volumes.iter().any(|item| &item.name == name));
                    self.catalog_version += 1;
                }
                Err(e) => self.storage_error = Some(e.to_string()),
            }
        }
        let preview = match &self.cleanup {
            Cleanup::Loading(rx) => rx.try_recv().ok(),
            _ => None,
        };
        if let Some(result) = preview {
            changed = true;
            self.cleanup = if self.review_cancelled {
                Cleanup::Idle
            } else {
                match result {
                    Ok(plan) => Cleanup::Review(plan),
                    Err(e) => Cleanup::Result(
                        format!("Review failed: {e}. No resources were deleted."),
                        true,
                    ),
                }
            };
        }
        let completed = match &self.cleanup {
            Cleanup::Running(rx) => rx.try_recv().ok(),
            _ => None,
        };
        if let Some(result) = completed {
            changed = true;
            self.cleanup = match result {
                Ok(text) => {
                    self.selected_volumes.clear();
                    Cleanup::Result(text, false)
                }
                Err(e) => Cleanup::Result(e.to_string(), true),
            };
            self.load_storage();
        }
        for run in &mut self.runs {
            let messages: Vec<_> = run
                .job
                .as_ref()
                .map(|job| job.rx.try_iter().collect())
                .unwrap_or_default();
            for message in messages {
                changed = true;
                match message {
                    LaunchMessage::Line(text, error) => {
                        append_log(&mut run.lines, &mut run.lines_bytes, text, error);
                    }
                    LaunchMessage::Done(result) => {
                        self.history.push(ObservedEvent {
                            at: SystemTime::now(),
                            resource: run.key.clone(),
                            project: Some(run.key.clone()),
                            message: result.as_ref().cloned().unwrap_or_else(|e| e.clone()),
                            warning: result.is_err(),
                        });
                        run.result = Some(result);
                        run.job = None;
                    }
                }
            }
        }
        changed
    }
}

/// OSC 52 is supported by terminals that allow clipboard escape sequences.
pub fn clipboard_escape(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        encoded.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        encoded.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    format!("\x1b]52;c;{encoded}\x07")
}
