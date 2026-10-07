//! Bounded, session-local observations. Missing samples never become zeroes.
use super::projects::{Project, Resource};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

pub const HISTORY_LIMIT: usize = 2000;
const SAMPLE_LIMIT: usize = 300;
const SERIES_LIMIT: usize = 1024;
#[derive(Clone)]
pub struct ObservedEvent {
    pub at: SystemTime,
    pub resource: String,
    pub project: Option<String>,
    pub message: String,
    pub warning: bool,
}
#[derive(Clone, Copy)]
pub struct Metric {
    pub at: SystemTime,
    pub cpu: f32,
    pub memory: u64,
    pub estimated: bool,
}
#[derive(Default)]
pub struct History {
    pub events: VecDeque<ObservedEvent>,
    pub samples: HashMap<String, VecDeque<Metric>>,
    observed: HashMap<String, (Option<String>, String)>,
    native: HashMap<String, (Option<String>, String)>,
    pub started: Option<SystemTime>,
}
impl History {
    pub fn push(&mut self, event: ObservedEvent) {
        self.started.get_or_insert(event.at);
        self.events.push_back(event);
        while self.events.len() > HISTORY_LIMIT {
            self.events.pop_front();
        }
    }
    pub fn sample(&mut self, key: String, metric: Metric) {
        if !self.samples.contains_key(&key) && self.samples.len() >= SERIES_LIMIT {
            if let Some(oldest) = self
                .samples
                .iter()
                .min_by_key(|(_, samples)| samples.back().map(|m| m.at))
                .map(|(key, _)| key.clone())
            {
                self.samples.remove(&oldest);
            }
        }
        let samples = self.samples.entry(key).or_default();
        samples.push_back(metric);
        if samples.len() > SAMPLE_LIMIT {
            samples.pop_front();
        }
    }
    pub fn observe(&mut self, projects: &[Project], at: SystemTime) {
        let mut observed = HashMap::new();
        for project in projects {
            if project.unmeasured == 0 {
                self.sample(
                    project.key.clone(),
                    Metric {
                        at,
                        cpu: project.cpu,
                        memory: project.memory,
                        estimated: project.estimated,
                    },
                );
            }
            for record in &project.resources {
                let key = record.resource.key();
                if let (Some(cpu), Some(memory)) = (record.cpu, record.memory) {
                    if !matches!(record.resource, Resource::Process { .. }) {
                        self.sample(
                            key.clone(),
                            Metric {
                                at,
                                cpu,
                                memory,
                                estimated: record.estimated,
                            },
                        );
                    }
                }
                if matches!(record.resource, Resource::Pm2 { .. }) {
                    let status = record
                        .details
                        .iter()
                        .find(|s| s.starts_with("Status:"))
                        .cloned()
                        .unwrap_or_else(|| "present".into());
                    if let Some((_, old_status)) = self.observed.get(&key) {
                        if old_status != &status {
                            self.push(ObservedEvent {
                                at,
                                resource: key.clone(),
                                project: Some(project.key.clone()),
                                message: format!("{} · {status}", record.name),
                                warning: status.contains("errored") || status.contains("stopped"),
                            });
                        }
                    } else if !self.observed.is_empty() {
                        self.push(ObservedEvent {
                            at,
                            resource: key.clone(),
                            project: Some(project.key.clone()),
                            message: format!("{} appeared in snapshot", record.name),
                            warning: false,
                        });
                    }
                    observed.insert(key, (Some(project.key.clone()), status));
                }
            }
        }
        let missing: Vec<_> = self
            .observed
            .iter()
            .filter(|(key, _)| !observed.contains_key(*key))
            .map(|(key, (project, _))| (key.clone(), project.clone()))
            .collect();
        for (resource, project) in missing {
            self.push(ObservedEvent {
                at,
                resource,
                project,
                message: "Process disappeared from snapshot; exit cause unavailable".into(),
                warning: true,
            });
        }
        self.observed = observed;
    }
    pub fn native_snapshot(
        &mut self,
        entries: &[crate::system::process::ProcessEntry],
        projects: &[Project],
        at: SystemTime,
    ) {
        let project_for: HashMap<_, _> = projects
            .iter()
            .flat_map(|p| {
                p.resources
                    .iter()
                    .filter(|r| matches!(r.resource, Resource::Process { .. }))
                    .map(|r| (r.resource.key(), p.key.clone()))
            })
            .collect();
        let mut observed = HashMap::new();
        for entry in entries.iter().filter(|e| !e.is_thread && e.start_time != 0) {
            let key = Resource::Process {
                pid: entry.pid.as_u32(),
                identity: entry.start_time,
            }
            .key();
            let project = project_for.get(&key).cloned();
            self.sample(
                key.clone(),
                Metric {
                    at,
                    cpu: entry.cpu,
                    memory: entry
                        .memory_sample
                        .map_or(entry.memory_bytes, |m| m.pss_bytes),
                    estimated: entry.memory_sample.is_none(),
                },
            );
            if !self.native.is_empty() && !self.native.contains_key(&key) {
                self.push(ObservedEvent {
                    at,
                    resource: key.clone(),
                    project: project.clone(),
                    message: format!("{} (PID {}) appeared in snapshot", entry.name, entry.pid),
                    warning: false,
                });
            }
            observed.insert(key, (project, entry.name.clone()));
        }
        let missing: Vec<_> = self
            .native
            .iter()
            .filter(|(key, _)| !observed.contains_key(*key))
            .map(|(key, (project, name))| (key.clone(), project.clone(), name.clone()))
            .collect();
        for (resource, project, name) in missing {
            self.push(ObservedEvent {
                at,
                resource,
                project,
                message: format!("{name} disappeared from snapshot; exit cause unavailable"),
                warning: true,
            });
        }
        self.native = observed;
    }
    pub fn docker_event(&mut self, line: &str, projects: &[Project]) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let kind = value["Type"].as_str().unwrap_or("container");
        if kind != "container" && kind != "volume" {
            return;
        }
        let action = value["Action"]
            .as_str()
            .or_else(|| value["status"].as_str())
            .unwrap_or("event");
        let id = value["Actor"]["ID"]
            .as_str()
            .or_else(|| value["id"].as_str())
            .unwrap_or("");
        if id.is_empty() {
            return;
        }
        let attributes = &value["Actor"]["Attributes"];
        let name = attributes["name"].as_str().unwrap_or(id);
        let project = projects
            .iter()
            .find(|p| {
                p.resources.iter().any(|r| match &r.resource {
                    Resource::Container(container) => {
                        container == id || (container.len() >= 12 && id.starts_with(container))
                    }
                    Resource::Volume(volume) => kind == "volume" && volume == id,
                    _ => false,
                })
            })
            .map(|p| p.key.clone())
            .or_else(|| {
                attributes["com.docker.compose.project"]
                    .as_str()
                    .and_then(|name| {
                        projects
                            .iter()
                            .find(|p| p.name == name)
                            .map(|p| p.key.clone())
                    })
            });
        let code = attributes["exitCode"]
            .as_str()
            .map(|code| format!(" · exit {code}"))
            .unwrap_or_default();
        let at = value["time"]
            .as_u64()
            .and_then(|secs| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs)))
            .unwrap_or_else(SystemTime::now);
        self.push(ObservedEvent {
            at,
            resource: format!("{kind}:{id}"),
            project,
            message: format!("{name} · {action}{code}"),
            warning: action == "oom"
                || action.contains("unhealthy")
                || (action == "die" && attributes["exitCode"].as_str() != Some("0")),
        });
    }
    pub fn for_resource(&self, resource: &Resource) -> Vec<&ObservedEvent> {
        let key = resource.key();
        self.events
            .iter()
            .filter(|e| match resource {
                Resource::Project(_) => e.project.as_deref() == Some(&key),
                Resource::Container(id) => {
                    e.resource == key
                        || e.resource
                            .strip_prefix("container:")
                            .is_some_and(|full| id.len() >= 12 && full.starts_with(id))
                }
                _ => e.resource == key,
            })
            .collect()
    }
}
pub fn clock(at: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(at)
        .format("%H:%M:%S")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn docker_exit_and_health_events_keep_cause_and_scope() {
        let mut history = History::default();
        history.docker_event(r#"{"Type":"container","Action":"die","Actor":{"ID":"abc","Attributes":{"name":"api","exitCode":"137"}},"time":10}"#,&[]);
        let event = history.events.back().unwrap();
        assert!(event.warning);
        assert!(event.message.contains("exit 137"));
        assert_eq!(event.resource, "container:abc");
        history.docker_event("not json", &[]);
        assert_eq!(history.events.len(), 1);
    }
    #[test]
    fn history_is_bounded_and_samples_retain_estimates() {
        let mut history = History::default();
        for _ in 0..HISTORY_LIMIT + 10 {
            history.push(ObservedEvent {
                at: SystemTime::now(),
                resource: "x".into(),
                project: None,
                message: "x".into(),
                warning: false,
            });
        }
        assert_eq!(history.events.len(), HISTORY_LIMIT);
        for _ in 0..SAMPLE_LIMIT + 10 {
            history.sample(
                "x".into(),
                Metric {
                    at: SystemTime::now(),
                    cpu: 5.0,
                    memory: 10,
                    estimated: true,
                },
            );
        }
        assert_eq!(history.samples["x"].len(), SAMPLE_LIMIT);
        assert!(history.samples["x"][0].estimated);
    }
}
