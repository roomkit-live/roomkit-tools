use std::collections::VecDeque;

use roomkit_tools_core::Error;

use crate::ProcessChunk;

#[derive(Debug)]
pub(crate) struct Journal {
    bytes: VecDeque<u8>,
    #[cfg(unix)]
    limit: usize,
    pub(crate) total: u64,
    pub(crate) eof: bool,
}

impl Journal {
    #[cfg(unix)]
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit,
            total: 0,
            eof: false,
        }
    }

    pub(crate) fn first(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }

    #[cfg(unix)]
    pub(crate) fn push(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.total = self
            .total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("process output cursor overflow"))?;
        for byte in bytes {
            if self.bytes.len() == self.limit {
                self.bytes.pop_front();
            }
            self.bytes.push_back(*byte);
        }
        Ok(())
    }

    pub(crate) fn page(&self, requested: u64, limit: usize) -> Result<ProcessChunk, Error> {
        if requested > self.total {
            return Err(Error::InvalidInput(
                "output cursor is past the observed stream end".into(),
            ));
        }
        let offset = requested.max(self.first());
        // offset - first is within the retained buffer's usize-sized length.
        let skip = usize::try_from(offset - self.first())
            .map_err(|_| Error::InvalidInput("invalid output cursor".into()))?;
        let bytes: Vec<u8> = self.bytes.iter().skip(skip).take(limit).copied().collect();
        let next_offset = offset + bytes.len() as u64;
        Ok(ProcessChunk {
            offset,
            next_offset,
            total_bytes: self.total,
            omitted_bytes: offset - requested,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            bytes,
            has_more: next_offset < self.total,
            eof: self.eof,
        })
    }
}
