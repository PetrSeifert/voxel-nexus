use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

type Job = Box<dyn FnOnce() + Send>;

pub(super) struct RasterWorkerPool {
    sender: Option<mpsc::Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

pub(super) struct RasterTask<T> {
    receiver: mpsc::Receiver<std::thread::Result<T>>,
}

impl<T> RasterTask<T> {
    pub(super) fn join(self) -> Result<T, ()> {
        self.receiver.recv().map_err(|_| ())?.map_err(|_| ())
    }
}

impl RasterWorkerPool {
    pub(super) fn new() -> std::io::Result<Self> {
        let region_workers = thread::available_parallelism()?.get().clamp(1, 8);
        Self::with_region_workers(region_workers)
    }

    pub(super) fn with_region_workers(region_workers: usize) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut pool = Self {
            sender: Some(sender),
            workers: Vec::new(),
        };
        // One worker coordinates a generation while the others derive its regions.
        for index in 0..=region_workers {
            let receiver = receiver.clone();
            pool.workers.push(
                thread::Builder::new()
                    .name(format!("raster-worker-{index}"))
                    .spawn(move || {
                        loop {
                            let job = match receiver.lock() {
                                Ok(receiver) => receiver.recv(),
                                Err(_) => return,
                            };
                            match job {
                                Ok(job) => job(),
                                Err(_) => return,
                            }
                        }
                    })?,
            );
        }
        Ok(pool)
    }

    pub(super) fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub(super) fn region_worker_count(&self) -> usize {
        self.workers.len().saturating_sub(1)
    }

    pub(super) fn execute<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> std::io::Result<RasterTask<T>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .ok_or_else(|| std::io::Error::other("raster worker pool stopped"))?
            .send(Box::new(move || {
                let result = catch_unwind(AssertUnwindSafe(job));
                if sender.send(result).is_err() {
                    eprintln!("raster worker task result receiver closed");
                }
            }))
            .map_err(|_| std::io::Error::other("raster worker pool unavailable"))?;
        Ok(RasterTask { receiver })
    }
}

impl Drop for RasterWorkerPool {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                eprintln!("raster pool worker panicked during shutdown");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn workers_run_concurrently_and_are_reused() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RasterWorkerPool::with_region_workers(2)?;
        let mut generations = Vec::new();
        for _ in 0..2 {
            let (started_sender, started_receiver) = mpsc::channel();
            let mut tasks = Vec::new();
            let mut releases = Vec::new();
            for _ in 0..3 {
                let (release_sender, release_receiver) = mpsc::channel();
                releases.push(release_sender);
                let started_sender = started_sender.clone();
                tasks.push(pool.execute(move || -> Result<(), String> {
                    started_sender
                        .send(thread::current().id())
                        .map_err(|error| error.to_string())?;
                    release_receiver
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| error.to_string())
                })?);
            }
            let mut identities = HashSet::new();
            for _ in 0..3 {
                identities.insert(started_receiver.recv_timeout(Duration::from_secs(10))?);
            }
            assert_eq!(identities.len(), 3);
            for release in releases {
                release.send(())?;
            }
            for task in tasks {
                task.join().map_err(|_| "task panicked")??;
            }
            generations.push(identities);
        }
        assert_eq!(generations.first(), generations.last());
        Ok(())
    }

    #[test]
    fn task_panic_is_reported_and_pool_can_run_another_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = RasterWorkerPool::with_region_workers(1)?;
        assert!(
            pool.execute(|| panic!("injected worker task failure"))?
                .join()
                .is_err()
        );
        assert_eq!(
            pool.execute(|| 42)?.join().map_err(|_| "worker failed")?,
            42
        );
        Ok(())
    }
}
