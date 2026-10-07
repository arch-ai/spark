//! Docker's existing stats stream supplies measurements without recurring CLI calls.
use super::ContainerInfo;
use crate::system::live::{LiveStream, StreamMessage};
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{Duration, Instant};

const STALE_AFTER: Duration = Duration::from_secs(15);
const MAX_SAMPLES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerMemory {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub percent: f64,
    pub measured_at: Instant,
    pub stale: bool,
}

fn parse_sample(line: &str, now: Instant) -> Option<(String, ContainerMemory)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = value.get("ID")?.as_str()?;
    // The command uses --no-trunc. Never attach measurements by name or ID prefix.
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (used, limit) = value.get("MemUsage")?.as_str()?.split_once('/')?;
    let used_bytes = crate::app::sorting::size_bytes(used)?;
    let limit_bytes = crate::app::sorting::size_bytes(limit)?;
    // Docker emits 0B / 0B before a measurement, and for stopped containers.
    if limit_bytes == 0 || used_bytes == u64::MAX || limit_bytes == u64::MAX {
        return None;
    }
    let percent = value
        .get("MemPerc")
        .and_then(|v| v.as_str())
        .and_then(|s| s.trim().strip_suffix('%'))
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .unwrap_or(used_bytes as f64 / limit_bytes as f64 * 100.0);
    Some((
        id.to_owned(),
        ContainerMemory {
            used_bytes,
            limit_bytes,
            percent,
            measured_at: now,
            stale: false,
        },
    ))
}

#[derive(Default)]
pub struct DockerMemory {
    samples: HashMap<String, ContainerMemory>,
    known: HashSet<String>,
    running: HashSet<String>,
    stream: Option<LiveStream>,
    attempted: bool,
    pub error: Option<String>,
}

impl DockerMemory {
    pub fn sync_containers(&mut self, containers: &[ContainerInfo]) {
        let running: HashSet<_> = containers
            .iter()
            .filter(|c| c.running)
            .map(|c| c.id.clone())
            .collect();
        // A stopped/restarted container must wait for a new sample, even with the same ID.
        self.samples.retain(|id, _| {
            running.contains(id) && (!self.known.contains(id) || self.running.contains(id))
        });
        self.known = containers.iter().map(|c| c.id.clone()).collect();
        self.running = running;
    }

    pub fn get(&self, id: &str) -> Option<ContainerMemory> {
        self.running
            .contains(id)
            .then(|| self.samples.get(id).copied())
            .flatten()
    }

    pub fn reconnect(&mut self) {
        self.stream = None;
        self.attempted = false;
        self.error = None;
        self.mark_stale();
    }

    fn mark_stale(&mut self) -> bool {
        let mut changed = false;
        for sample in self.samples.values_mut() {
            changed |= !sample.stale;
            sample.stale = true;
        }
        changed
    }

    fn ingest(&mut self, message: StreamMessage, now: Instant) -> bool {
        match message {
            StreamMessage::Line {
                text,
                stderr: false,
            } => {
                if let Some((id, sample)) = parse_sample(&text, now) {
                    if self.samples.contains_key(&id) || self.samples.len() < MAX_SAMPLES {
                        self.samples.insert(id, sample);
                        return true;
                    }
                }
            }
            StreamMessage::Line { text, stderr: true } => {
                self.error = Some(text.chars().take(500).collect());
                return true;
            }
            StreamMessage::Exit(result) => {
                let reason = match result {
                    Ok(status) => format!("stats stream ended ({status})"),
                    Err(error) => error.to_string(),
                };
                self.error = Some(match self.error.take() {
                    Some(error) => format!("{error}; {reason}"),
                    None => reason,
                });
                self.stream = None;
                self.mark_stale();
                return true;
            }
        }
        false
    }

    fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for sample in self.samples.values_mut() {
            if !sample.stale && now.saturating_duration_since(sample.measured_at) >= STALE_AFTER {
                sample.stale = true;
                changed = true;
            }
        }
        changed
    }

    /// Called by the existing UI loop. The pipe readers block waiting for Docker data.
    pub fn update(&mut self, active: bool) -> bool {
        if !active {
            let changed = self.stream.take().is_some() || self.attempted;
            self.attempted = false;
            return self.mark_stale() || changed;
        }
        let mut changed = false;
        if !self.attempted {
            self.attempted = true;
            self.error = None;
            match LiveStream::spawn(Command::new("docker").args([
                "stats",
                "--no-trunc",
                "--format",
                "{{json .}}",
            ])) {
                Ok(stream) => self.stream = Some(stream),
                Err(error) => self.error = Some(error.to_string()),
            }
            changed = true;
        }
        let messages = self
            .stream
            .as_ref()
            .map(LiveStream::drain)
            .unwrap_or_default();
        let now = Instant::now();
        for message in messages {
            changed |= self.ingest(message, now);
        }
        changed | self.expire(now)
    }

    pub fn notice(&self) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(format!("RAM stats: {error} · F5 retry"));
        }
        self.running
            .iter()
            .any(|id| self.samples.get(id).is_some_and(|m| m.stale))
            .then(|| "RAM * = cached measurement · F5 refresh".into())
    }

    pub fn details(&self, id: &str) -> Vec<String> {
        if self.known.contains(id) && !self.running.contains(id) {
            return vec!["Memory: not running (no live measurement)".into()];
        }
        let mut lines = match self.get(id) {
            Some(sample) => vec![
                format!(
                    "Memory: {} / {} ({:.2}% of limit)",
                    bytes(sample.used_bytes),
                    bytes(sample.limit_bytes),
                    sample.percent
                ),
                format!(
                    "Memory sample: {}s ago{}",
                    sample.measured_at.elapsed().as_secs(),
                    if sample.stale {
                        " · STALE (last known)"
                    } else {
                        " · live"
                    }
                ),
                "Docker CLI memory excludes inactive file cache on Linux.".into(),
            ],
            None => vec!["Memory: unavailable · waiting for a valid live sample".into()],
        };
        if let Some(error) = &self.error {
            lines.push(format!("Memory stats: {error} · F5 retry"));
        }
        lines
    }
}

fn bytes(value: u64) -> String {
    for (unit, scale) in [
        ("TiB", 1u64 << 40),
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
    ] {
        if value >= scale {
            return format!("{:.2} {unit}", value as f64 / scale as f64);
        }
    }
    format!("{value} B")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(id: &str, memory: &str, percent: &str) -> String {
        serde_json::json!({"ID":id,"MemUsage":memory,"MemPerc":percent}).to_string()
    }
    #[test]
    fn parses_binary_and_decimal_units_and_real_zero_without_accepting_missing_data() {
        let id = "a".repeat(64);
        let now = Instant::now();
        let sample = parse_sample(&line(&id, "1.5GiB / 2GiB", "75.00%"), now)
            .unwrap()
            .1;
        assert_eq!(sample.used_bytes, 3 << 29);
        assert_eq!(sample.limit_bytes, 2 << 30);
        assert_eq!(sample.percent, 75.0);
        assert_eq!(
            parse_sample(&line(&id, "50 MB / 100 MB", ""), now)
                .unwrap()
                .1
                .percent,
            50.0
        );
        assert_eq!(
            parse_sample(&line(&id, "0B / 2GiB", "0.00%"), now)
                .unwrap()
                .1
                .used_bytes,
            0
        );
        for input in [
            line(&id, "0B / 0B", "0%"),
            line(&id, "N/A / 2GiB", "0%"),
            line("aaaaaaaaaaaa", "1MiB / 2MiB", "50%"),
            "invalid".into(),
        ] {
            assert!(parse_sample(&input, now).is_none());
        }
    }

    #[test]
    fn stalled_and_failed_streams_keep_cached_values_marked_stale() {
        use std::os::unix::process::ExitStatusExt;
        let id = "a".repeat(64);
        let now = Instant::now();
        let mut stats = DockerMemory::default();
        stats.running.insert(id.clone());
        stats.ingest(
            StreamMessage::Line {
                text: line(&id, "512MiB / 2GiB", "25%"),
                stderr: false,
            },
            now,
        );
        assert!(!stats.get(&id).unwrap().stale);
        assert!(stats.expire(now + STALE_AFTER));
        assert!(stats.get(&id).unwrap().stale);
        stats.ingest(
            StreamMessage::Line {
                text: line(&id, "1GiB / 2GiB", "50%"),
                stderr: false,
            },
            now,
        );
        assert!(!stats.get(&id).unwrap().stale);
        stats.ingest(
            StreamMessage::Exit(Ok(std::process::ExitStatus::from_raw(256))),
            now,
        );
        assert_eq!(stats.get(&id).unwrap().used_bytes, 1 << 30);
        assert!(stats.get(&id).unwrap().stale);
        assert!(stats.notice().unwrap().contains("F5 retry"));
    }

    #[test]
    fn stopped_removed_and_restarted_containers_cannot_inherit_an_old_reading() {
        let id = "a".repeat(64);
        let mut container = ContainerInfo {
            id: id.clone(),
            name: "api".into(),
            image: "demo".into(),
            port_public: "-".into(),
            port_internal: "-".into(),
            status: "Up".into(),
            group_name: "demo".into(),
            group_path: None,
            running: true,
            memory: None,
            activity_secs: 0,
        };
        let mut stats = DockerMemory::default();
        let sample = || StreamMessage::Line {
            text: line(&id, "512MiB / 2GiB", "25%"),
            stderr: false,
        };
        // Stats can arrive before the first container-list response.
        stats.ingest(sample(), Instant::now());
        assert!(stats.get(&id).is_none());
        stats.sync_containers(&[container.clone()]);
        assert!(stats.get(&id).is_some());
        container.running = false;
        stats.sync_containers(&[container.clone()]);
        assert!(stats.get(&id).is_none());
        assert!(stats.details(&id)[0].contains("not running"));
        // A queued stats line may arrive after metadata reports the stop.
        stats.ingest(sample(), Instant::now());
        assert!(stats.get(&id).is_none());
        container.running = true;
        stats.sync_containers(&[container]);
        assert!(stats.get(&id).is_none());
        stats.ingest(sample(), Instant::now());
        assert!(stats.get(&id).is_some());
        stats.sync_containers(&[]);
        assert!(stats.get(&id).is_none());
    }
}
