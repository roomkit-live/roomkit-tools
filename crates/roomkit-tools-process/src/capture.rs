#[cfg(unix)]
use std::collections::VecDeque;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::io::{AsyncRead, AsyncReadExt};

/// One stream, retaining its beginning and most recent bytes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CapturedOutput {
    /// UTF-8 view; invalid or split sequences become replacement characters.
    pub text: String,
    /// Whether the retention budget discarded bytes.
    pub truncated: bool,
    /// Bytes discarded between the retained beginning and end.
    pub omitted_bytes: u64,
    /// Bytes actually read, including discarded bytes.
    pub total_bytes: u64,
}

#[cfg(unix)]
pub(crate) struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    limit: usize,
    total: u64,
}

#[cfg(unix)]
impl Capture {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            limit,
            total: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let head_remaining = self.limit.div_ceil(2).saturating_sub(self.head.len());
        let (head, tail) = bytes.split_at(bytes.len().min(head_remaining));
        self.head.extend_from_slice(head);
        let tail_limit = self.limit / 2;
        for byte in tail {
            if tail_limit != 0 {
                if self.tail.len() == tail_limit {
                    self.tail.pop_front();
                }
                self.tail.push_back(*byte);
            }
        }
    }

    pub(crate) fn finish(self) -> CapturedOutput {
        let retained = self.head.len() + self.tail.len();
        let omitted = self.total.saturating_sub(retained as u64);
        let tail = self.tail.into_iter().collect::<Vec<_>>();
        let text = if omitted == 0 {
            let mut bytes = self.head;
            bytes.extend(tail);
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            format!(
                "{}\n[... {omitted} bytes omitted ...]\n{}",
                String::from_utf8_lossy(&self.head),
                String::from_utf8_lossy(&tail)
            )
        };
        CapturedOutput {
            text,
            truncated: omitted != 0,
            omitted_bytes: omitted,
            total_bytes: self.total,
        }
    }

    pub(crate) async fn drain(
        &mut self,
        stream: &mut (impl AsyncRead + Unpin),
    ) -> std::io::Result<()> {
        let mut bytes = vec![0; 8192];
        loop {
            let count = stream.read(&mut bytes).await?;
            if count == 0 {
                return Ok(());
            }
            if let Some(chunk) = bytes.get(..count) {
                self.push(chunk);
            }
        }
    }
}
