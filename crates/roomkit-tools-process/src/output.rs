//! Optional, owner-scoped disk copies of raw process output.

mod types;
mod writer;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use roomkit_tools_core::{Definition, Output, ToolError};
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

pub use types::{
    OutputArchive, OutputForgetInput, OutputForgetResult, OutputLimits, OutputPage, OutputReadInput,
};
pub(crate) use writer::{Sink, View};

#[derive(Debug)]
struct Record {
    owner: String,
    state: Mutex<OutputArchive>,
    file: Mutex<Option<Arc<File>>>,
    directory: Arc<TempDir>,
    reserved: AtomicU64,
    capture_done: AtomicBool,
    budget: Arc<AtomicU64>,
    closed: CancellationToken,
    done: watch::Sender<bool>,
}

impl Drop for Record {
    fn drop(&mut self) {
        self.budget
            .fetch_sub(self.reserved.load(Ordering::Relaxed), Ordering::AcqRel);
    }
}

impl Record {
    fn snapshot(&self) -> OutputArchive {
        let mut state = lock(&self.state).clone();
        state.complete = state.finished
            && state.eof
            && state.error.is_none()
            && state.stored_bytes == state.observed_bytes;
        state
    }

    fn fail(&self, reason: &str) {
        lock(&self.state).error.get_or_insert_with(|| reason.into());
    }

    async fn wait(&self) {
        let mut done = self.done.subscribe();
        while !*done.borrow_and_update() {
            if done.changed().await.is_err() {
                break;
            }
        }
    }

    fn release(&self) {
        lock(&self.file).take();
        self.budget
            .fetch_sub(self.reserved.swap(0, Ordering::AcqRel), Ordering::AcqRel);
    }
}

#[derive(Debug, Default)]
struct Registry {
    next: u64,
    entries: BTreeMap<String, Arc<Record>>,
}

#[derive(Debug)]
struct Inner {
    directory: Arc<TempDir>,
    limits: OutputLimits,
    budget: Arc<AtomicU64>,
    registry: Mutex<Registry>,
    closed: CancellationToken,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

/// Optional ephemeral output storage shared by selected shell/process tools.
///
/// Every archive belongs to the owner of the command that produced it, a
/// trusted key the host passes, never from model arguments; `None` is the
/// one standalone owner, that of [`ShellTool::shell`](crate::ShellTool::shell).
/// Clones share a store.
/// Copies survive process-record deletion until explicit forget/store close.
/// The last store reference requests writer cleanup; prefer explicit close.
/// No restart persistence, fsync durability, sandbox or arbitrary file access.
#[derive(Clone, Debug)]
pub struct OutputStore {
    inner: Arc<Inner>,
}

/// A new directory that only the host user can list, enter or write (0700),
/// set when it is created rather than changed afterwards.
fn private_directory(parent: &Path) -> std::io::Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("roomkit-output-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn append_metadata(
    text: &mut String,
    stdout: Option<&OutputArchive>,
    stderr: Option<&OutputArchive>,
) {
    for (name, archive) in [("stdout", stdout), ("stderr", stderr)] {
        if let Some(archive) = archive {
            let _written = write!(
                text,
                "\n{name} archive: id={:?}, stored={}, observed={}, eof={}, finished={}, complete={}, error={:?}; use output_read to read retained bytes\n",
                archive.archive_id,
                archive.stored_bytes,
                archive.observed_bytes,
                archive.eof,
                archive.finished,
                archive.complete,
                archive.error
            );
        }
    }
}

impl OutputStore {
    /// Create a private temporary subdirectory (mode 0700) beneath a host-selected
    /// parent, or the system temporary directory for `None`, as Python's
    /// `OutputStore(parent=None)`.
    ///
    /// # Errors
    /// Rejects invalid bounds, inaccessible parents and unsupported platforms.
    pub async fn new(parent: Option<&Path>, limits: OutputLimits) -> Result<Self, ToolError> {
        if !cfg!(unix) {
            return Err(ToolError::UnsupportedResource(
                "output archives currently require Unix".into(),
            ));
        }
        if limits.max_archives == 0
            || limits.max_archives_per_owner == 0
            || limits.max_archives_per_owner > limits.max_archives
            || limits.max_stream_bytes == 0
            || limits.max_total_bytes == 0
            || limits.max_read_bytes == 0
            || limits.write_timeout_ms == 0
            || limits.write_timeout_ms > 60_000
        {
            return Err(ToolError::InvalidInput(
                "invalid output store limits".into(),
            ));
        }
        let parent = parent.map_or_else(std::env::temp_dir, Path::to_path_buf);
        let directory = tokio::task::spawn_blocking(move || private_directory(&parent))
            .await
            .map_err(|error| ToolError::Io {
                operation: "join output store creation",
                source: std::io::Error::other(error),
            })?
            .map_err(|source| ToolError::Io {
                operation: "create output store",
                source,
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                directory: Arc::new(directory),
                limits,
                budget: Arc::new(AtomicU64::new(0)),
                registry: Mutex::new(Registry::default()),
                closed: CancellationToken::new(),
            }),
        })
    }

    /// Whether close was requested. Closed stores cannot reopen.
    pub fn closed(&self) -> bool {
        self.inner.closed.is_cancelled()
    }

    pub(crate) fn write_timeout_ms(&self) -> u64 {
        self.inner.limits.write_timeout_ms
    }

    /// Derived model input and result schemas for paged archive reads.
    pub fn read_definition() -> Definition {
        Definition::new::<OutputReadInput, OutputPage>(
            "output_read",
            "Read a page of a retained command output by archive_id. Does not rerun the command or consume bytes. Check archive.complete/error; a partial copy is only a prefix. Use next_offset for more stored bytes. IDs belong to this conversation, not filesystem paths.",
        )
    }

    /// Derived schemas for explicitly releasing a finished output copy.
    pub fn forget_definition() -> Definition {
        Definition::new::<OutputForgetInput, OutputForgetResult>(
            "output_forget",
            "Permanently release one owned output archive after capture and writing finish. Frees its quota; the identifier becomes invalid. Does not stop a running process. Only forget when the retained log is no longer needed.",
        )
    }

    fn entry(
        &self,
        owner: &str,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<Arc<Record>, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if self.closed() {
            return Err(ToolError::AccessRefused("output store is closed".into()));
        }
        lock(&self.inner.registry)
            .entries
            .get(id)
            .filter(|entry| entry.owner == owner)
            .cloned()
            .ok_or_else(|| ToolError::AccessRefused("unknown output archive".into()))
    }

    /// List only the trusted owner's retained copies, including failed/active
    /// ones, in creation order.
    ///
    /// Async as Python's `OutputStore.list`; the listing itself waits for
    /// nothing, so the future is ready when the call returns.
    ///
    /// # Errors
    /// Returns cancellation or refusal after close.
    pub fn list(
        &self,
        owner: Option<&str>,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Vec<OutputArchive>, ToolError>> {
        std::future::ready(self.snapshots(crate::owner_key(owner), cancel))
    }

    fn snapshots(
        &self,
        owner: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<OutputArchive>, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if self.closed() {
            return Err(ToolError::AccessRefused("output store is closed".into()));
        }
        let registry = lock(&self.inner.registry);
        let mut records: Vec<_> = registry
            .entries
            .iter()
            .filter(|(_, record)| record.owner == owner)
            .collect();
        records.sort_by_key(|&(id, _)| roomkit_tools_core::identifiers::creation_order(id));
        Ok(records
            .into_iter()
            .map(|(_, record)| record.snapshot())
            .collect())
    }

    /// Read confirmed bytes only; changes after the snapshot appear on a later call.
    ///
    /// # Errors
    /// Rejects foreign/unknown IDs and offsets past stored bytes. Reports read I/O failures.
    pub async fn read(
        &self,
        owner: Option<&str>,
        input: OutputReadInput,
        cancel: &CancellationToken,
    ) -> Result<Output<OutputPage>, ToolError> {
        let owner = crate::owner_key(owner);
        let record = self.entry(owner, &input.archive_id, cancel)?;
        let archive = record.snapshot();
        if input.offset > archive.stored_bytes {
            return Err(ToolError::InvalidInput(
                "offset is past stored output".into(),
            ));
        }
        let count = (archive.stored_bytes - input.offset).min(
            input
                .max_bytes
                .map_or(self.inner.limits.max_read_bytes, NonZeroUsize::get)
                .min(self.inner.limits.max_read_bytes) as u64,
        );
        let file = lock(&record.file).clone();
        let offset = input.offset;
        let bytes = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            result = tokio::task::spawn_blocking(move || writer::read(file.as_deref(), offset, count)) => result
                .map_err(|error| ToolError::Io { operation: "join output read", source: std::io::Error::other(error) })?
                .map_err(|source| ToolError::Io { operation: "read output archive", source })?,
        };
        let next_offset = offset + bytes.len() as u64;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let rendered = format!(
            "{} bytes {}..{} (complete={}, error={:?}):\n{}",
            input.archive_id, offset, next_offset, archive.complete, archive.error, text
        );
        Ok(Output::new(
            OutputPage {
                has_more: next_offset < archive.stored_bytes,
                archive,
                offset,
                next_offset,
                bytes,
                text,
            },
            rendered,
        ))
    }

    /// Release a finished record. In-flight reads may keep its bytes alive briefly.
    ///
    /// # Errors
    /// Rejects active, unknown or foreign copies, closed stores and cancellation.
    pub async fn forget(
        &self,
        owner: Option<&str>,
        input: OutputForgetInput,
        cancel: &CancellationToken,
    ) -> Result<Output<OutputForgetResult>, ToolError> {
        let owner = crate::owner_key(owner);
        let record = self.entry(owner, &input.archive_id, cancel)?;
        let state = record.snapshot();
        if !state.finished || !record.capture_done.load(Ordering::Acquire) {
            return Err(ToolError::AccessRefused(
                "output capture or writing is still active".into(),
            ));
        }
        lock(&self.inner.registry).entries.remove(&input.archive_id);
        record.fail("forgotten");
        tokio::task::spawn_blocking(move || record.release())
            .await
            .map_err(|error| ToolError::Io {
                operation: "release output archive",
                source: std::io::Error::other(error),
            })?;
        let text = format!("{} forgotten\n", input.archive_id);
        Ok(Output::new(
            OutputForgetResult {
                archive_id: input.archive_id,
                forgotten: true,
            },
            text,
        ))
    }

    /// Request shutdown, await writers for at most two seconds and remove retained files.
    /// Returns false while filesystem work is still pending; another close may wait again.
    pub async fn close(&self) -> bool {
        self.inner.closed.cancel();
        let records: Vec<_> = lock(&self.inner.registry)
            .entries
            .values()
            .cloned()
            .collect();
        let finished = tokio::time::timeout(Duration::from_secs(2), async {
            for record in &records {
                record.wait().await;
            }
        })
        .await
        .is_ok();
        if finished {
            for record in &records {
                record.fail("store_closed");
                record.release();
            }
            lock(&self.inner.registry).entries.clear();
            return match tokio::fs::remove_dir(self.inner.directory.path()).await {
                Ok(()) => true,
                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            };
        }
        finished
    }

    pub(crate) fn begin(&self, owner: &str) -> Sink {
        let mut registry = lock(&self.inner.registry);
        let error = if self.closed() {
            Some("store_closed")
        } else if registry.entries.len() >= self.inner.limits.max_archives
            || registry
                .entries
                .values()
                .filter(|r| r.owner == owner)
                .count()
                >= self.inner.limits.max_archives_per_owner
        {
            Some("archive_limit")
        } else if registry.next == u64::MAX {
            Some("identifier_limit")
        } else {
            None
        };
        let id = if error.is_none() {
            registry.next += 1;
            Some(format!("o{}", registry.next))
        } else {
            None
        };
        let (done, _) = watch::channel(error.is_some());
        let record = Arc::new(Record {
            owner: owner.into(),
            state: Mutex::new(OutputArchive {
                archive_id: id.clone(),
                observed_bytes: 0,
                stored_bytes: 0,
                eof: false,
                finished: error.is_some(),
                complete: false,
                error: error.map(String::from),
            }),
            file: Mutex::new(None),
            directory: Arc::clone(&self.inner.directory),
            reserved: AtomicU64::new(0),
            capture_done: AtomicBool::new(false),
            budget: Arc::clone(&self.inner.budget),
            closed: self.inner.closed.child_token(),
            done,
        });
        let sender = id.map(|id| {
            registry.entries.insert(id, Arc::clone(&record));
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(writer::run(Arc::clone(&record), rx));
            tx
        });
        Sink::new(record, sender, self.inner.limits)
    }
}
