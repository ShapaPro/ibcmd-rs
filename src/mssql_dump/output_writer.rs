//! The export's file writes, done by a few threads of their own.
//!
//! The export converts rows on a pool of workers, and every worker used to
//! write its own files: creating a folder and a file is a wait on the disk,
//! and on a busy disk it held the workers as long as the conversion itself
//! (ERP УХ 8.3.27: 200-470 thread-seconds for the forms' `Form.xml` alone).
//! Here a worker hands the finished bytes to a small pool of writer threads
//! and goes on converting.
//!
//! - Memory stays bounded: what is queued and not yet written is held under
//!   a byte and a count budget, and a worker that would exceed it waits for
//!   the writers. Nothing larger than one file ever waits beyond the budget.
//! - Every file is written exactly as before: the same bytes to the same
//!   path, its folder created first. Writes to one path keep their order
//!   (each path always goes to the same writer thread).
//! - A failed write fails the export: every later call reports it, and
//!   [`OutputWriter::finish`] returns it.
//! - `IBCMD_RS_OUTPUT_WRITERS=0` writes on the calling thread, as before.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};

use crate::cli::InfobaseConfigSourceVersion;

/// Writer threads when `IBCMD_RS_OUTPUT_WRITERS` does not say.
const DEFAULT_WRITERS: usize = 4;
/// Bytes queued and not yet written, at most (`IBCMD_RS_OUTPUT_WRITE_BUDGET_MB`).
const DEFAULT_BUDGET_BYTES: usize = 256 << 20;
/// Files queued and not yet written, at most.
const MAX_QUEUED_JOBS: usize = 8192;

/// Bytes a file gets, taken without a copy when the caller owns them.
pub(crate) trait OutputBytes {
    fn into_output_bytes(self) -> Vec<u8>;
}

impl OutputBytes for Vec<u8> {
    fn into_output_bytes(self) -> Vec<u8> {
        self
    }
}

impl OutputBytes for String {
    fn into_output_bytes(self) -> Vec<u8> {
        self.into_bytes()
    }
}

impl OutputBytes for &[u8] {
    fn into_output_bytes(self) -> Vec<u8> {
        self.to_vec()
    }
}

impl OutputBytes for &Vec<u8> {
    fn into_output_bytes(self) -> Vec<u8> {
        self.clone()
    }
}

impl OutputBytes for &str {
    fn into_output_bytes(self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl OutputBytes for &String {
    fn into_output_bytes(self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

enum Job {
    Write { path: PathBuf, bytes: Vec<u8> },
    CreateDir { path: PathBuf },
}

impl Job {
    fn weight(&self) -> usize {
        match self {
            Job::Write { bytes, .. } => bytes.len(),
            Job::CreateDir { .. } => 0,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Job::Write { path, .. } | Job::CreateDir { path } => path,
        }
    }
}

/// What the writers did, for the timing report.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OutputWriteStats {
    pub(crate) threads: u64,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    /// Time the writer threads spent writing (summed over them).
    pub(crate) busy_ms: u64,
    /// Time the converting workers waited for room in the queue.
    pub(crate) wait_ms: u64,
    /// Wall time [`OutputWriter::finish`] waited for the queue to drain.
    pub(crate) drain_ms: u64,
}

struct Queue {
    bytes: usize,
    jobs: usize,
}

struct Shared {
    queue: Mutex<Queue>,
    room: Condvar,
    max_bytes: usize,
    max_jobs: usize,
    failed: AtomicBool,
    failure: Mutex<Option<anyhow::Error>>,
    /// Folders known to exist: every file creates its own folder first, and
    /// most files share theirs with others.
    folders: Mutex<HashSet<PathBuf>>,
    files: AtomicU64,
    bytes: AtomicU64,
    busy_ns: AtomicU64,
    wait_ns: AtomicU64,
}

impl Shared {
    fn record_failure(&self, error: anyhow::Error) {
        let mut failure = self
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if failure.is_none() {
            *failure = Some(error);
        }
        self.failed.store(true, Ordering::SeqCst);
    }

    fn failure_message(&self) -> String {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|error| format!("{error:#}"))
            .unwrap_or_else(|| "a writer thread stopped".to_string())
    }

    /// Creates `folder` unless it is known to exist.
    fn ensure_folder(&self, folder: &Path) -> Result<()> {
        if folder.as_os_str().is_empty() {
            return Ok(());
        }
        {
            let folders = self
                .folders
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if folders.contains(folder) {
                return Ok(());
            }
        }
        fs::create_dir_all(folder)
            .with_context(|| format!("failed to create {}", folder.display()))?;
        self.folders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(folder.to_path_buf());
        Ok(())
    }

    fn run(&self, job: &Job) -> Result<()> {
        match job {
            Job::Write { path, bytes } => {
                if let Some(parent) = path.parent() {
                    self.ensure_folder(parent)?;
                }
                fs::write(path, bytes)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                self.files.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Ok(())
            }
            Job::CreateDir { path } => self.ensure_folder(path),
        }
    }

    /// Waits for room for a job of `weight` bytes and takes it.
    fn reserve(&self, weight: usize) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let started = Instant::now();
        let mut waited = false;
        // A job larger than the whole budget still goes through, alone.
        while queue.jobs > 0
            && (queue.bytes + weight > self.max_bytes || queue.jobs + 1 > self.max_jobs)
        {
            waited = true;
            queue = self
                .room
                .wait(queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        queue.bytes += weight;
        queue.jobs += 1;
        drop(queue);
        if waited {
            self.wait_ns
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }

    fn release(&self, weight: usize) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.bytes -= weight;
        queue.jobs -= 1;
        drop(queue);
        self.room.notify_all();
    }
}

/// The export's files, written by a small pool of threads (or inline).
pub(crate) struct OutputWriter {
    senders: Vec<Sender<Job>>,
    handles: Vec<JoinHandle<()>>,
    threads: usize,
    shared: Arc<Shared>,
}

impl OutputWriter {
    /// A writer sized by `IBCMD_RS_OUTPUT_WRITERS` (threads, `0` = inline)
    /// and `IBCMD_RS_OUTPUT_WRITE_BUDGET_MB`.
    pub(crate) fn from_env() -> Self {
        let threads = std::env::var("IBCMD_RS_OUTPUT_WRITERS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_WRITERS);
        let budget = std::env::var("IBCMD_RS_OUTPUT_WRITE_BUDGET_MB")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .map(|megabytes| megabytes.max(1) << 20)
            .unwrap_or(DEFAULT_BUDGET_BYTES);
        Self::new(threads, budget)
    }

    /// Writes on the calling thread: every call is the write itself.
    pub(crate) fn inline() -> Self {
        Self::new(0, DEFAULT_BUDGET_BYTES)
    }

    pub(crate) fn new(threads: usize, budget_bytes: usize) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue { bytes: 0, jobs: 0 }),
            room: Condvar::new(),
            max_bytes: budget_bytes.max(1),
            max_jobs: MAX_QUEUED_JOBS,
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            folders: Mutex::new(HashSet::new()),
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            busy_ns: AtomicU64::new(0),
            wait_ns: AtomicU64::new(0),
        });
        let mut senders = Vec::with_capacity(threads);
        let mut handles = Vec::with_capacity(threads);
        for index in 0..threads {
            let (sender, receiver) = mpsc::channel::<Job>();
            let shared = Arc::clone(&shared);
            let handle = std::thread::Builder::new()
                .name(format!("ibcmd-output-writer-{index}"))
                .spawn(move || {
                    for job in receiver {
                        let weight = job.weight();
                        if !shared.failed.load(Ordering::SeqCst) {
                            let started = Instant::now();
                            // A panic is a failure like any other, and the job's
                            // budget is released whatever happened: a worker
                            // waiting for room must never wait for a writer
                            // that is gone.
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    shared.run(&job)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow!("writing {} panicked", job.path().display()))
                                });
                            if let Err(error) = result {
                                shared.record_failure(error);
                            }
                            shared
                                .busy_ns
                                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        }
                        shared.release(weight);
                    }
                });
            match handle {
                Ok(handle) => {
                    senders.push(sender);
                    handles.push(handle);
                }
                // No thread for this writer: the others (or the caller) write.
                Err(_) => break,
            }
        }
        let threads = handles.len();
        Self {
            senders,
            handles,
            threads,
            shared,
        }
    }

    /// Writes `bytes` to `path`, creating its folder.
    pub(crate) fn write(&self, path: impl Into<PathBuf>, bytes: impl OutputBytes) -> Result<()> {
        self.submit(Job::Write {
            path: path.into(),
            bytes: bytes.into_output_bytes(),
        })
    }

    /// A source XML file: the bytes as `write_source_xml_file` writes them.
    pub(crate) fn write_xml(
        &self,
        path: impl Into<PathBuf>,
        xml: impl AsRef<[u8]>,
        source_version: InfobaseConfigSourceVersion,
    ) -> Result<()> {
        self.write(
            path,
            super::source_assets::source_xml_file_bytes(xml, source_version),
        )
    }

    /// Creates `path` and its parents.
    pub(crate) fn create_dir_all(&self, path: impl Into<PathBuf>) -> Result<()> {
        self.submit(Job::CreateDir { path: path.into() })
    }

    fn submit(&self, job: Job) -> Result<()> {
        if self.shared.failed.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "an earlier output write failed: {}",
                self.shared.failure_message()
            ));
        }
        if self.senders.is_empty() {
            let started = Instant::now();
            let result = self.shared.run(&job);
            self.shared
                .busy_ns
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            return result;
        }
        let mut hasher = DefaultHasher::new();
        job.path().hash(&mut hasher);
        let index = (hasher.finish() % self.senders.len() as u64) as usize;
        let weight = job.weight();
        self.shared.reserve(weight);
        if self.senders[index].send(job).is_err() {
            self.shared.release(weight);
            return Err(anyhow!(
                "output writer {index} stopped: {}",
                self.shared.failure_message()
            ));
        }
        Ok(())
    }

    /// Waits for every queued write; the first failure, if any.
    pub(crate) fn finish(mut self) -> Result<OutputWriteStats> {
        let started = Instant::now();
        let panicked = self.join();
        let drain_ms = started.elapsed().as_millis() as u64;
        if let Some(error) = self
            .shared
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            return Err(error);
        }
        if panicked {
            return Err(anyhow!("an output writer thread panicked"));
        }
        Ok(OutputWriteStats {
            threads: self.threads as u64,
            files: self.shared.files.load(Ordering::Relaxed),
            bytes: self.shared.bytes.load(Ordering::Relaxed),
            busy_ms: self.shared.busy_ns.load(Ordering::Relaxed) / 1_000_000,
            wait_ms: self.shared.wait_ns.load(Ordering::Relaxed) / 1_000_000,
            drain_ms,
        })
    }

    /// Closes the queues and waits for the threads; whether one panicked.
    fn join(&mut self) -> bool {
        self.senders.clear();
        let mut panicked = false;
        for handle in self.handles.drain(..) {
            panicked |= handle.join().is_err();
        }
        panicked
    }
}

impl Drop for OutputWriter {
    fn drop(&mut self) {
        // An export that stops early still lets what it queued land.
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ibcmd-output-writer-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn writes_every_file_with_its_folder_under_a_small_budget() {
        let root = scratch("files");
        for threads in [0, 1, 3] {
            let out = root.join(format!("t{threads}"));
            // A budget smaller than one file: each still goes through, alone.
            let writer = OutputWriter::new(threads, 10);
            for index in 0..200 {
                let path = out
                    .join(format!("d{}", index % 7))
                    .join(format!("f{index}.txt"));
                writer
                    .write(path, format!("file {index} {}", "x".repeat(index)))
                    .unwrap();
            }
            writer
                .create_dir_all(out.join("empty").join("nested"))
                .unwrap();
            // The same path twice: the later bytes win.
            writer
                .write(out.join("same.txt"), b"first".as_slice())
                .unwrap();
            writer
                .write(out.join("same.txt"), b"second".as_slice())
                .unwrap();
            let stats = writer.finish().unwrap();
            assert_eq!(stats.files, 202);
            for index in 0..200 {
                let path = out
                    .join(format!("d{}", index % 7))
                    .join(format!("f{index}.txt"));
                assert_eq!(
                    fs::read_to_string(path).unwrap(),
                    format!("file {index} {}", "x".repeat(index))
                );
            }
            assert!(out.join("empty").join("nested").is_dir());
            assert_eq!(fs::read_to_string(out.join("same.txt")).unwrap(), "second");
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_failed_write_fails_the_later_calls_and_the_finish() {
        let root = scratch("failure");
        // A file where a folder has to be: its children cannot be written.
        fs::write(root.join("blocker"), b"not a folder").unwrap();
        for threads in [0, 2] {
            let writer = OutputWriter::new(threads, DEFAULT_BUDGET_BYTES);
            let first = writer.write(root.join("blocker").join("child.txt"), b"x".as_slice());
            if threads == 0 {
                assert!(first.is_err());
                continue;
            }
            first.unwrap();
            // Later calls report the failure once a writer has seen it.
            let mut refused = false;
            for index in 0..1000 {
                if writer
                    .write(root.join(format!("ok{index}.txt")), b"y".as_slice())
                    .is_err()
                {
                    refused = true;
                    break;
                }
                std::thread::yield_now();
            }
            let finished = writer.finish();
            assert!(finished.is_err());
            let message = format!("{:#}", finished.unwrap_err());
            assert!(message.contains("blocker"), "{message}");
            let _ = refused;
        }
        fs::remove_dir_all(&root).ok();
    }
}

impl super::MssqlDumpTimingReport {
    /// What an export's writer did, into its table's timings.
    pub(crate) fn add_output_write(&mut self, stats: &OutputWriteStats) {
        self.output_write_threads = self.output_write_threads.max(stats.threads);
        self.output_write_files += stats.files;
        self.output_write_bytes += stats.bytes;
        self.output_write_busy_ms += stats.busy_ms;
        self.output_write_wait_ms += stats.wait_ms;
        self.output_write_drain_ms += stats.drain_ms;
    }
}
