//! Linux proportional memory accounting, sampled off the UI thread.
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use sysinfo::Pid;

use super::ProcessEntry;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SAMPLE_AGE: Duration = Duration::from_secs(15);
const SCAN_BUDGET: Duration = Duration::from_millis(50);
const MAX_READS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySample {
    pub pss_bytes: u64,
    pub swap_pss_bytes: Option<u64>,
}

struct CachedSample {
    checked_at: Instant,
    sample: Option<MemorySample>,
}

#[derive(Default)]
pub(super) struct MemorySampler {
    samples: HashMap<(Pid, u64), CachedSample>,
}

impl MemorySampler {
    pub fn refresh(&mut self, entries: &mut [ProcessEntry]) {
        let identities: HashMap<_, _> = entries
            .iter()
            .map(|entry| (entry.pid, entry.start_time))
            .collect();
        self.refresh_with(entries, Instant::now(), |pid| {
            let expected = identities[&pid];
            if super::process_identity(pid.as_u32())? != expected {
                return Err(io::ErrorKind::NotFound.into());
            }
            let sample = read_memory(pid)?;
            if super::process_identity(pid.as_u32())? != expected {
                return Err(io::ErrorKind::NotFound.into());
            }
            Ok(sample)
        });
    }

    fn refresh_with(
        &mut self,
        entries: &mut [ProcessEntry],
        now: Instant,
        mut read: impl FnMut(Pid) -> io::Result<MemorySample>,
    ) {
        let active: std::collections::HashSet<_> = entries
            .iter()
            .filter(|entry| !entry.is_thread)
            .map(|entry| (entry.pid, entry.start_time))
            .collect();
        self.samples.retain(|key, _| active.contains(key));
        let mut pending: Vec<_> = entries
            .iter()
            .filter(|entry| !entry.is_thread)
            .filter(|entry| {
                self.samples
                    .get(&(entry.pid, entry.start_time))
                    .is_none_or(|cached| {
                        now.saturating_duration_since(cached.checked_at) >= SAMPLE_INTERVAL
                    })
            })
            .collect();
        // Refresh expired measurements before newcomers. Otherwise a busy host
        // with process churn can leave every existing tree using RSS forever.
        pending.sort_by_key(|entry| {
            let cached = self.samples.get(&(entry.pid, entry.start_time));
            let expired = cached.is_some_and(|cached| {
                now.saturating_duration_since(cached.checked_at) >= MAX_SAMPLE_AGE
            });
            (
                !expired,
                cached.map(|cached| cached.checked_at),
                std::cmp::Reverse(entry.memory_bytes),
                entry.pid,
            )
        });
        let started = Instant::now();
        for entry in pending.into_iter().take(MAX_READS) {
            if started.elapsed() >= SCAN_BUDGET {
                break;
            }
            self.samples.insert(
                (entry.pid, entry.start_time),
                CachedSample {
                    checked_at: now,
                    sample: read(entry.pid).ok(),
                },
            );
        }
        for entry in entries {
            entry.memory_sample = self
                .samples
                .get(&(entry.pid, entry.start_time))
                .filter(|cached| now.saturating_duration_since(cached.checked_at) <= MAX_SAMPLE_AGE)
                .and_then(|cached| cached.sample);
        }
    }
}

#[cfg(target_os = "linux")]
fn read_memory(pid: Pid) -> io::Result<MemorySample> {
    let text = std::fs::read_to_string(format!("/proc/{}/smaps_rollup", pid.as_u32()))?;
    parse_rollup(&text)
}

#[cfg(not(target_os = "linux"))]
fn read_memory(_pid: Pid) -> io::Result<MemorySample> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "PSS requires Linux",
    ))
}

fn parse_rollup(text: &str) -> io::Result<MemorySample> {
    let mut pss = None;
    let mut swap = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let target = match fields.next() {
            Some("Pss:") => &mut pss,
            Some("SwapPss:") => &mut swap,
            _ => continue,
        };
        let value = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|kb| kb.checked_mul(1024));
        if value.is_none() || fields.next() != Some("kB") || target.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid smaps_rollup memory field",
            ));
        }
        *target = value;
    }
    Ok(MemorySample {
        pss_bytes: pss
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PSS unavailable"))?,
        swap_pss_bytes: swap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_values_use_kibibytes_and_do_not_add_pss_subfields() {
        let sample = parse_rollup("Rss: 100 kB\nPss: 60 kB\nPss_Anon: 40 kB\nPss_File: 20 kB\nSwap: 30 kB\nSwapPss: 15 kB\n").unwrap();
        assert_eq!(sample.pss_bytes, 60 * 1024);
        assert_eq!(sample.swap_pss_bytes, Some(15 * 1024));
        assert_eq!(parse_rollup("Pss: 0 kB").unwrap().pss_bytes, 0);
        for text in [
            "Rss: 10 kB",
            "Pss: bad kB",
            "Pss: 3 MB",
            "Pss: 18446744073709551615 kB",
            "Pss: 1 kB\nPss: 1 kB",
        ] {
            assert!(parse_rollup(text).is_err());
        }
    }

    #[test]
    fn cache_retries_failures_and_invalidates_reused_pids() {
        let mut sampler = MemorySampler::default();
        let now = Instant::now();
        let mut entries = vec![super::super::tests::entry(10, None, "app", 100)];
        sampler.refresh_with(&mut entries, now, |_| {
            Ok(MemorySample {
                pss_bytes: 60,
                swap_pss_bytes: Some(0),
            })
        });
        assert_eq!(entries[0].memory_sample.unwrap().pss_bytes, 60);
        sampler.refresh_with(&mut entries, now + Duration::from_secs(2), |_| {
            panic!("must use cache")
        });
        entries[0].start_time += 1;
        sampler.refresh_with(&mut entries, now + Duration::from_secs(2), |_| {
            Err(io::ErrorKind::PermissionDenied.into())
        });
        assert!(entries[0].memory_sample.is_none());
        assert_eq!(sampler.samples.len(), 1);
        sampler.refresh_with(&mut entries, now + Duration::from_secs(8), |_| {
            Ok(MemorySample {
                pss_bytes: 70,
                swap_pss_bytes: None,
            })
        });
        assert_eq!(entries[0].memory_sample.unwrap().pss_bytes, 70);
        sampler.refresh_with(&mut [], now, |_| unreachable!());
        assert!(sampler.samples.is_empty());
    }

    #[test]
    fn sampling_is_bounded_and_process_churn_cannot_starve_expired_samples() {
        let now = Instant::now();
        let mut sampler = MemorySampler::default();
        let mut entries: Vec<_> = (10..310)
            .map(|pid| super::super::tests::entry(pid, None, "app", 100))
            .collect();
        let stale = &entries[0];
        sampler.samples.insert(
            (stale.pid, stale.start_time),
            CachedSample {
                checked_at: now - Duration::from_secs(16),
                sample: Some(MemorySample {
                    pss_bytes: 50,
                    swap_pss_bytes: Some(0),
                }),
            },
        );
        let mut reads = 0;
        sampler.refresh_with(&mut entries, now, |_| {
            reads += 1;
            Ok(MemorySample {
                pss_bytes: 60,
                swap_pss_bytes: Some(0),
            })
        });
        assert!(reads > 0 && reads <= MAX_READS);
        // Process churn cannot starve an already expired measurement.
        assert_eq!(entries[0].memory_sample.unwrap().pss_bytes, 60);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn current_linux_process_has_readable_proportional_memory() {
        let sample = read_memory(Pid::from_u32(std::process::id())).unwrap();
        assert!(sample.pss_bytes > 0);
        assert!(sample.swap_pss_bytes.is_some());
    }
}
