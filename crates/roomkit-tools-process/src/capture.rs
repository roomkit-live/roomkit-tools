#[cfg(unix)]
use std::collections::VecDeque;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::io::{AsyncRead, AsyncReadExt};

/// One stream, retaining its most recent bytes: where a command reports how it
/// ended.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CapturedOutput {
    /// UTF-8 view; invalid or split sequences become replacement characters.
    pub text: String,
    /// Whether the retention budget discarded bytes.
    pub truncated: bool,
    /// Bytes discarded before the retained end.
    pub omitted_bytes: u64,
    /// Bytes actually read, including discarded bytes.
    pub total_bytes: u64,
    /// Optional disk copy. Absent when the host did not enable output storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<crate::OutputArchive>,
}

#[cfg(unix)]
pub(crate) struct Capture {
    tail: VecDeque<u8>,
    limit: usize,
    total: u64,
    archive: Option<crate::output::Sink>,
}

#[cfg(unix)]
impl Capture {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            tail: VecDeque::new(),
            limit,
            total: 0,
            archive: None,
        }
    }

    pub(crate) fn archive(&mut self, archive: Option<crate::output::Sink>) {
        self.archive = archive;
    }

    /// Release the disk copy of an output kept whole: it holds nothing the
    /// result lacks, and would only take one of its owner's archives.
    pub(crate) fn discard_whole_archive(&mut self, store: &crate::OutputStore) {
        if self.total <= self.tail.len() as u64
            && let Some(sink) = self.archive.take()
        {
            store.discard(sink);
        }
    }

    /// Write the block a cut-short drain left pending, then finish the copy.
    pub(crate) async fn finish_archive(&mut self) {
        if let Some(archive) = &mut self.archive {
            archive.flush_pending().await;
            archive.finish().await;
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let kept = bytes
            .get(bytes.len().saturating_sub(self.limit)..)
            .unwrap_or(bytes);
        self.tail.extend(kept);
        let excess = self.tail.len().saturating_sub(self.limit);
        self.tail.drain(..excess);
    }

    pub(crate) fn finish(self) -> CapturedOutput {
        let omitted = self.total.saturating_sub(self.tail.len() as u64);
        let tail = self.tail.into_iter().collect::<Vec<_>>();
        let kept = String::from_utf8_lossy(&tail);
        let text = if omitted == 0 {
            kept.into_owned()
        } else {
            format!("[... {omitted} earlier bytes omitted ...]\n{kept}")
        };
        CapturedOutput {
            text,
            truncated: omitted != 0,
            omitted_bytes: omitted,
            total_bytes: self.total,
            archive: self.archive.as_ref().map(crate::output::Sink::snapshot),
        }
    }

    pub(crate) async fn drain(
        &mut self,
        stream: &mut (impl AsyncRead + Unpin),
    ) -> std::io::Result<()> {
        let mut bytes = vec![0; 8192];
        loop {
            if let Some(archive) = &mut self.archive {
                archive.flush_pending().await;
            }
            let count = stream.read(&mut bytes).await?;
            if count == 0 {
                if let Some(archive) = &mut self.archive {
                    archive.end(true);
                }
                return Ok(());
            }
            if let Some(chunk) = bytes.get(..count) {
                self.push(chunk);
                if let Some(archive) = &mut self.archive {
                    archive.push(chunk);
                }
            }
        }
    }
}
