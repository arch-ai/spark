use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

pub struct Snapshot<T> {
    pub data: Arc<T>,
    pub error: Option<String>,
    pub updated_at: Option<Instant>,
}

/// One request at a time; wakes immediately on resume and sleeps while hidden.
pub struct DockerWorker<T> {
    data: Arc<RwLock<Arc<Snapshot<T>>>>,
    paused: Arc<AtomicBool>,
    wake: mpsc::SyncSender<()>,
}

impl<T> DockerWorker<T> {
    pub fn snapshot(&self) -> Arc<Snapshot<T>> {
        Arc::clone(&self.data.read().unwrap_or_else(|err| err.into_inner()))
    }

    pub fn set_paused(&self, paused: bool) {
        if self.paused.swap(paused, Ordering::Relaxed) != paused {
            self.refresh();
        }
    }

    pub fn refresh(&self) {
        // Coalesce repeated refresh requests instead of accumulating work.
        let _ = self.wake.try_send(());
    }
}

pub fn start_worker<T: Default + Send + Sync + 'static>(
    interval: Duration,
    load: impl Fn() -> io::Result<T> + Send + 'static,
) -> DockerWorker<T> {
    let data = Arc::new(RwLock::new(Arc::new(Snapshot {
        data: Arc::new(T::default()),
        error: None,
        updated_at: None,
    })));
    let paused = Arc::new(AtomicBool::new(true));
    let (wake, rx) = mpsc::sync_channel(1);
    let thread_data = Arc::clone(&data);
    let thread_paused = Arc::clone(&paused);
    thread::spawn(move || loop {
        if thread_paused.load(Ordering::Relaxed) {
            if rx.recv().is_err() {
                break;
            }
            if thread_paused.load(Ordering::Relaxed) {
                continue;
            }
        }
        let result = load();
        let mut guard = thread_data.write().unwrap_or_else(|err| err.into_inner());
        *guard = Arc::new(match result {
            Ok(data) => Snapshot {
                data: Arc::new(data),
                error: None,
                updated_at: Some(Instant::now()),
            },
            Err(err) => Snapshot {
                data: Arc::clone(&guard.data),
                error: Some(err.to_string()),
                updated_at: guard.updated_at,
            },
        });
        drop(guard);
        match rx.recv_timeout(interval) {
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            _ => continue,
        }
    });
    DockerWorker { data, paused, wake }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_preserve_the_last_good_snapshot_and_recovery_clears_them() {
        let (reply, replies) = mpsc::channel();
        let (entered, calls) = mpsc::channel();
        let worker = start_worker(Duration::from_secs(60), move || {
            let _ = entered.send(());
            replies
                .recv()
                .unwrap_or_else(|_| Err(io::Error::other("test finished")))
        });
        worker.set_paused(false);
        calls.recv_timeout(Duration::from_secs(1)).unwrap();
        reply.send(Ok(vec![1u8])).unwrap();
        worker.refresh();
        calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let good = worker.snapshot();
        assert_eq!(*good.data, vec![1]);
        reply.send(Err(io::Error::other("offline"))).unwrap();
        worker.refresh();
        calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let failed = worker.snapshot();
        assert!(Arc::ptr_eq(&failed.data, &good.data));
        assert_eq!(failed.updated_at, good.updated_at);
        assert_eq!(failed.error.as_deref(), Some("offline"));
        reply.send(Ok(vec![2])).unwrap();
        worker.refresh();
        calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let recovered = worker.snapshot();
        assert_eq!(*recovered.data, vec![2]);
        assert!(recovered.error.is_none());
        reply.send(Ok(Vec::new())).unwrap();
    }

    #[test]
    fn worker_starts_paused_and_wakes_without_waiting_for_interval() {
        let (tx, rx) = mpsc::channel();
        let worker = start_worker(Duration::from_secs(60), move || {
            tx.send(()).unwrap();
            Ok(Vec::<u8>::new())
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        worker.set_paused(false);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.set_paused(true);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        worker.set_paused(false);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }
}
