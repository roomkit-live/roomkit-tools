use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{OutputArchive, OutputLimits, Record, lock};

/// One pipe producer. Pending bytes survive cancellation of a drain future.
#[derive(Debug)]
pub(crate) struct Sink {
    record: Arc<Record>,
    sender: Option<mpsc::Sender<Vec<u8>>>,
    pending: Option<Vec<u8>>,
    limits: OutputLimits,
    ended: bool,
}

impl Sink {
    pub(super) fn new(
        record: Arc<Record>,
        sender: Option<mpsc::Sender<Vec<u8>>>,
        limits: OutputLimits,
    ) -> Self {
        Self {
            record,
            sender,
            pending: None,
            limits,
            ended: false,
        }
    }

    pub(crate) fn snapshot(&self) -> OutputArchive {
        self.record.snapshot()
    }

    pub(crate) fn view(&self) -> View {
        View(Arc::clone(&self.record))
    }

    /// Account for every byte even after copying was disabled. Capture uses 8 KiB chunks.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        let mut state = lock(&self.record.state);
        state.observed_bytes = state.observed_bytes.saturating_add(bytes.len() as u64);
        if self.record.closed.is_cancelled() {
            state.error.get_or_insert_with(|| "store_closed".into());
        }
        if self.sender.is_none() || state.error.is_some() {
            return;
        }
        let available = self
            .limits
            .max_stream_bytes
            .saturating_sub(self.record.reserved.load(Ordering::Relaxed));
        let wanted = available.min(bytes.len() as u64);
        let mut reserved = self.record.budget.load(Ordering::Relaxed);
        let count = loop {
            let count = wanted.min(self.limits.max_total_bytes.saturating_sub(reserved));
            match self.record.budget.compare_exchange_weak(
                reserved,
                reserved + count,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break count,
                Err(actual) => reserved = actual,
            }
        };
        self.record.reserved.fetch_add(count, Ordering::Relaxed);
        if count < bytes.len() as u64 {
            state.error = Some(
                if count < wanted {
                    "total_limit"
                } else {
                    "stream_limit"
                }
                .into(),
            );
        }
        // count cannot exceed this already-allocated slice's usize length.
        if let Ok(count) = usize::try_from(count) {
            self.pending = bytes
                .get(..count)
                .filter(|chunk| !chunk.is_empty())
                .map(<[u8]>::to_vec);
        }
    }

    /// Wait outside all registry/journal locks. On timeout, stop copying but keep draining.
    pub(crate) async fn flush_pending(&mut self) {
        let (Some(_), Some(sender)) = (&self.pending, &self.sender) else {
            return;
        };
        let result = tokio::select! {
            biased;
            () = self.record.closed.cancelled() => Err("store_closed"),
            result = tokio::time::timeout(Duration::from_millis(self.limits.write_timeout_ms), sender.reserve()) => match result {
                Ok(Ok(permit)) => { if let Some(bytes) = self.pending.take() { permit.send(bytes); } Ok(()) },
                Ok(Err(_)) => Err("write_failed"),
                Err(_) => Err("write_timeout"),
            },
        };
        if let Err(reason) = result {
            self.record.fail(reason);
            self.pending.take();
            self.sender.take();
        }
    }

    pub(crate) fn end(&mut self, eof: bool) {
        self.ended = true;
        self.record.capture_done.store(true, Ordering::Release);
        lock(&self.record.state).eof = eof;
        if !eof || self.pending.is_some() {
            self.record.fail("capture_incomplete");
        }
        self.pending.take();
        self.sender.take();
    }

    pub(crate) async fn finish(&mut self) {
        if !self.ended {
            self.end(false);
        }
        if tokio::time::timeout(
            Duration::from_millis(self.limits.write_timeout_ms),
            self.record.wait(),
        )
        .await
        .is_err()
        {
            self.record.fail("write_timeout");
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        if !self.ended {
            self.end(false);
        }
    }
}

/// Read-only handle for dynamic process metadata; it never owns a sender.
#[derive(Clone, Debug)]
pub(crate) struct View(Arc<Record>);

impl View {
    pub(crate) fn snapshot(&self) -> OutputArchive {
        self.0.snapshot()
    }
    pub(crate) async fn wait(&self, timeout_ms: u64) {
        if tokio::time::timeout(Duration::from_millis(timeout_ms), self.0.wait())
            .await
            .is_err()
        {
            self.0.fail("write_timeout");
        }
    }
}

pub(super) async fn run(record: Arc<Record>, receiver: mpsc::Receiver<Vec<u8>>) {
    let directory = Arc::clone(&record.directory);
    let file = tokio::task::spawn_blocking(move || tempfile::tempfile_in(directory.path())).await;
    match file {
        Ok(Ok(file)) => copy(&record, receiver, file).await,
        _ => record.fail("create_failed"),
    }
    lock(&record.state).finished = true;
    record.done.send_replace(true);
}

async fn copy(record: &Arc<Record>, mut receiver: mpsc::Receiver<Vec<u8>>, file: File) {
    let file = Arc::new(file);
    *lock(&record.file) = Some(Arc::clone(&file));
    while let Some(bytes) = tokio::select! {
        biased;
        () = record.closed.cancelled() => { record.fail("store_closed"); None },
        bytes = receiver.recv() => bytes,
    } {
        let offset = lock(&record.state).stored_bytes;
        let file = Arc::clone(&file);
        let result = tokio::task::spawn_blocking(move || write(&file, &bytes, offset)).await;
        if let Ok((count, failed)) = result {
            lock(&record.state).stored_bytes += count as u64;
            if failed {
                record.fail("write_failed");
                break;
            }
        } else {
            record.fail("write_failed");
            break;
        }
    }
}

fn write(file: &File, bytes: &[u8], offset: u64) -> (usize, bool) {
    let mut written = 0;
    while let Some(remaining) = bytes.get(written..).filter(|v| !v.is_empty()) {
        #[cfg(unix)]
        let result = file.write_at(remaining, offset + written as u64);
        #[cfg(not(unix))]
        let result = {
            let _ = (file, offset, remaining);
            Err::<usize, _>(std::io::Error::other("unsupported platform"))
        };
        match result {
            Ok(0) => return (written, true),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return (written, true),
        }
    }
    (written, false)
}

pub(super) fn read(file: Option<&File>, offset: u64, count: u64) -> std::io::Result<Vec<u8>> {
    let count = usize::try_from(count).map_err(|_| std::io::Error::other("invalid page size"))?;
    let mut bytes = vec![0; count];
    let mut total = 0;
    while let Some(remaining) = bytes.get_mut(total..).filter(|v| !v.is_empty()) {
        let file = file.ok_or_else(|| std::io::Error::other("archive file unavailable"))?;
        #[cfg(unix)]
        let result = file.read_at(remaining, offset + total as u64);
        #[cfg(not(unix))]
        let result = {
            let _ = (file, offset, remaining);
            Err::<usize, _>(std::io::Error::other("unsupported platform"))
        };
        match result {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "short output archive",
                ));
            }
            Ok(count) => total += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::task::Poll;
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    #[expect(clippy::unwrap_used, reason = "test fixture creation must succeed")]
    fn fixture() -> (Sink, mpsc::Receiver<Vec<u8>>) {
        let (done, _) = watch::channel(false);
        let record = Arc::new(Record {
            owner: Some("test".into()),
            state: Mutex::new(OutputArchive {
                archive_id: Some("o1".into()),
                observed_bytes: 0,
                stored_bytes: 0,
                eof: false,
                finished: false,
                complete: false,
                error: None,
            }),
            file: Mutex::new(None),
            directory: Arc::new(tempfile::tempdir().unwrap()),
            reserved: AtomicU64::new(0),
            capture_done: AtomicBool::new(false),
            budget: Arc::new(AtomicU64::new(0)),
            closed: CancellationToken::new(),
            done,
        });
        let (tx, rx) = mpsc::channel(1);
        (Sink::new(record, Some(tx), OutputLimits::default()), rx)
    }

    #[tokio::test]
    async fn a_cancelled_queue_wait_keeps_its_pending_chunk() {
        let (mut sink, mut receiver) = fixture();
        sink.push(b"first");
        sink.flush_pending().await;
        sink.push(b"second");
        {
            let mut pending = std::pin::pin!(sink.flush_pending());
            std::future::poll_fn(|cx| {
                assert!(pending.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        assert_eq!(receiver.recv().await.unwrap(), b"first");
        sink.flush_pending().await;
        assert_eq!(receiver.recv().await.unwrap(), b"second");
        assert_eq!(sink.snapshot().observed_bytes, 11);
        assert_eq!(sink.snapshot().error, None);
    }

    #[tokio::test]
    async fn full_writer_queue_disables_copying_with_an_explicit_error() {
        let (mut sink, mut receiver) = fixture();
        sink.limits.write_timeout_ms = 1;
        sink.push(b"first");
        sink.flush_pending().await;
        sink.push(b"second");
        sink.flush_pending().await;
        assert_eq!(sink.snapshot().error.as_deref(), Some("write_timeout"));
        sink.push(b"still observed");
        assert_eq!(sink.snapshot().observed_bytes, 25);
        assert_eq!(receiver.recv().await.unwrap(), b"first");
        assert_eq!(receiver.recv().await, None);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn disk_write_failure_reports_only_confirmed_bytes() {
        let (mut sink, receiver) = fixture();
        sink.push(b"data");
        sink.flush_pending().await;
        sink.end(true);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        copy(&sink.record, receiver, file).await;
        let state = sink.snapshot();
        assert_eq!(state.stored_bytes, 0);
        assert_eq!(state.observed_bytes, 4);
        assert_eq!(state.error.as_deref(), Some("write_failed"));
        assert!(!state.complete);
    }
}
