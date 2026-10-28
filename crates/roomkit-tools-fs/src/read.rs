use std::num::NonZeroUsize;
use std::path::Path;

use crate::paths::Root;
use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

const READ_CHUNK_BYTES: usize = 64 * 1024;

/// A request to read a page of UTF-8 text.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    /// A path relative to the tool's root, or an absolute path within it.
    pub path: String,
    /// The first line to return, starting at one. Defaults to one.
    #[serde(default = "first_line")]
    pub offset: NonZeroUsize,
    /// Requested line count, capped by the host's limit. Omit for that limit.
    #[serde(default)]
    pub limit: Option<NonZeroUsize>,
}

impl ReadInput {
    /// Read `path` from its first line, using the host's default page limit.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            offset: first_line(),
            limit: None,
        }
    }
}

const fn first_line() -> NonZeroUsize {
    NonZeroUsize::MIN
}

/// Host-owned bounds, independent of model-supplied arguments.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ReadLimits {
    /// Maximum bytes scanned from one file. Default: 8 MiB. Memory is page-bounded;
    /// raise this limit explicitly to read larger files.
    pub max_file_bytes: usize,
    /// Maximum bytes in the numbered text returned to the model. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum lines per page. Default: 2,000.
    pub max_lines: usize,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_output_bytes: 50 * 1024,
            max_lines: 2_000,
        }
    }
}

/// One complete line, without its line ending or a generated number prefix.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ReadLine {
    /// One-based line number in the source file.
    pub number: usize,
    /// The original text on this line.
    pub text: String,
}

/// A page of source text and the information needed to continue reading it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ReadPage {
    /// The resolved file path, formatted for display.
    pub path: String,
    /// Complete source lines, without generated prefixes.
    pub lines: Vec<ReadLine>,
    /// Whether more lines remain after this page.
    pub truncated: bool,
    /// First line of the next page, or `None` at EOF.
    pub next_offset: Option<usize>,
    /// SHA-256 revision of all raw file bytes, including BOM and line endings.
    /// Pass as `expected_revision` to write/edit to refuse stale changes.
    pub revision: String,
}

/// A read-only tool restricted to regular files beneath a host-selected root.
///
/// Canonicalization rejects existing symlinks that resolve outside the root.
/// This path check is not an OS sandbox or a guarantee against concurrent,
/// adversarial filesystem changes. The host must control the workspace.
#[derive(Clone, Debug)]
pub struct ReadTool {
    root: Root,
    limits: ReadLimits,
}

impl ReadTool {
    /// Resolve the host's root once, without changing the process directory.
    ///
    /// # Errors
    /// Returns an I/O failure when the root cannot be resolved, or invalid
    /// input when it is not a directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = Root::new(root).await?;
        Ok(Self {
            root,
            limits: ReadLimits::default(),
        })
    }

    /// Replace the host's bounds.
    ///
    /// # Errors
    /// Every bound must be positive and the ingestion limit must fit in `u64`
    /// with room for one extra byte to detect a file growing during the read.
    pub fn with_limits(mut self, limits: ReadLimits) -> Result<Self, Error> {
        if limits.max_file_bytes == 0
            || limits.max_output_bytes == 0
            || limits.max_lines == 0
            || u64::try_from(limits.max_file_bytes).map_or(true, |n| n == u64::MAX)
        {
            return Err(Error::InvalidInput(
                "read limits must be positive and representable".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// The tool's schemas, derived from its actual input and output types.
    pub fn definition() -> Definition {
        Definition::new::<ReadInput, ReadPage>(
            "read",
            "Read a page of UTF-8 text from a file inside the configured workspace. Lines are numbered from one. Large pages return next_offset. Returns a revision of the whole file; pass it as expected_revision to write/edit to refuse stale changes. The full file is scanned with bounded memory; binary files and files above the host's ingestion limit are refused.",
        )
    }

    /// Read the requested page, or stop waiting when `cancel` is cancelled.
    ///
    /// Tokio may finish an already-issued filesystem operation on its blocking
    /// pool after cancellation; this tool performs no writes.
    ///
    /// # Errors
    /// Returns an access refusal for paths outside the root, unsupported for
    /// non-regular or non-text files, a limit error for oversized files or a
    /// line that cannot fit in an empty page, and an I/O error on read failure.
    /// An offset beyond EOF is invalid, except line one of an empty file.
    pub async fn read(
        &self,
        input: ReadInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ReadPage>, Error> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(Error::Cancelled),
            result = self.read_file(input) => result,
        }
    }

    async fn read_file(&self, input: ReadInput) -> Result<Output<ReadPage>, Error> {
        let path = self.root.resolve(&input.path).await?;
        let info = fs::metadata(&path).await.map_err(|source| Error::Io {
            operation: "inspect the file",
            source,
        })?;
        if !info.is_file() {
            return Err(Error::Unsupported("read accepts regular files only".into()));
        }
        let maximum = u64::try_from(self.limits.max_file_bytes)
            .map_err(|_| Error::InvalidInput("ingestion limit is too large".into()))?;
        if info.len() > maximum {
            return Err(Error::LimitExceeded("file exceeds max_file_bytes".into()));
        }
        let file = File::open(&path).await.map_err(|source| Error::Io {
            operation: "open the file",
            source,
        })?;
        scan(file.take(maximum + 1), &path, &input, self.limits).await
    }
}

async fn scan(
    mut reader: impl AsyncRead + Unpin,
    path: &Path,
    input: &ReadInput,
    limits: ReadLimits,
) -> Result<Output<ReadPage>, Error> {
    let mut page = Page::new(input, limits);
    let mut hash = Sha256::new();
    let mut buffer = vec![0; READ_CHUNK_BYTES];
    let mut pending = 0;
    let mut ingested = 0;
    let mut first = true;
    loop {
        let count = reader
            .read(buffer.split_at_mut(pending).1)
            .await
            .map_err(|source| Error::Io {
                operation: "read the file",
                source,
            })?;
        if count > limits.max_file_bytes - ingested {
            return Err(Error::LimitExceeded("file exceeds max_file_bytes".into()));
        }
        ingested += count;
        let length = pending + count;
        let bytes = buffer.split_at(length).0;
        hash.update(bytes.split_at(pending).1);
        if bytes.contains(&0) {
            return Err(Error::Unsupported("file contains binary NUL bytes".into()));
        }
        let (text, complete) = match std::str::from_utf8(bytes) {
            Ok(text) => (text, length),
            Err(error) if error.error_len().is_none() && count > 0 => {
                let complete = error.valid_up_to();
                let text = std::str::from_utf8(bytes.split_at(complete).0)
                    .map_err(|_| Error::Unsupported("file is not UTF-8 text".into()))?;
                (text, complete)
            }
            Err(_) => return Err(Error::Unsupported("file is not UTF-8 text".into())),
        };
        if !text.is_empty() {
            let text = if first {
                first = false;
                text.strip_prefix('\u{feff}').unwrap_or(text)
            } else {
                text
            };
            page.push(text)?;
        }
        if count == 0 {
            break;
        }
        buffer.copy_within(complete..length, 0);
        pending = length - complete;
        // A buffered/in-memory reader may always be ready. Yield between bounded
        // chunks so cancellation can also interrupt long scans of such readers.
        tokio::task::yield_now().await;
    }
    page.finish(path, crate::revision::finish(hash))
}

struct Page {
    offset: usize,
    limit: usize,
    bytes: usize,
    number: usize,
    total: usize,
    current: String,
    line_open: bool,
    oversized: bool,
    lines: Vec<ReadLine>,
    text: String,
    next_offset: Option<usize>,
}

impl Page {
    fn new(input: &ReadInput, limits: ReadLimits) -> Self {
        Self {
            offset: input.offset.get(),
            limit: input
                .limit
                .map_or(limits.max_lines, NonZeroUsize::get)
                .min(limits.max_lines),
            bytes: limits.max_output_bytes,
            number: 1,
            total: 0,
            current: String::new(),
            line_open: false,
            oversized: false,
            lines: Vec::new(),
            text: String::new(),
            next_offset: None,
        }
    }

    fn push(&mut self, text: &str) -> Result<(), Error> {
        for part in text.split_inclusive('\n') {
            self.line_open = true;
            let fragment = part.strip_suffix('\n').unwrap_or(part);
            if self.number >= self.offset && self.next_offset.is_none() {
                if self.lines.len() >= self.limit {
                    self.next_offset = Some(self.number);
                } else if !self.oversized {
                    // Keep at most the output bound, even for a single huge line.
                    // Prefix/newline costs and CRLF removal are settled at its end.
                    if fragment.len() > self.bytes - self.text.len() - self.current.len() {
                        self.oversized = true;
                        self.current.clear();
                    } else {
                        self.current.push_str(fragment);
                    }
                }
            }
            if part.ends_with('\n') {
                if self.current.ends_with('\r') {
                    self.current.pop();
                }
                self.end_line()?;
            }
        }
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), Error> {
        if self.number >= self.offset && self.next_offset.is_none() {
            let prefix = format!("{}: ", self.number);
            let size = prefix
                .len()
                .saturating_add(self.current.len())
                .saturating_add(1);
            if self.oversized || size > self.bytes - self.text.len() {
                if self.lines.is_empty() {
                    return Err(Error::LimitExceeded(
                        "one line exceeds max_output_bytes".into(),
                    ));
                }
                self.next_offset = Some(self.number);
            } else {
                self.text.push_str(&prefix);
                self.text.push_str(&self.current);
                self.text.push('\n');
                self.lines.push(ReadLine {
                    number: self.number,
                    text: std::mem::take(&mut self.current),
                });
            }
        }
        self.current.clear();
        self.oversized = false;
        self.line_open = false;
        self.total = self.number;
        self.number = self
            .number
            .checked_add(1)
            .ok_or_else(|| Error::LimitExceeded("file has too many lines to represent".into()))?;
        Ok(())
    }

    fn finish(mut self, path: &Path, revision: String) -> Result<Output<ReadPage>, Error> {
        if self.line_open {
            self.end_line()?;
        }
        if self.offset > self.total && !(self.offset == 1 && self.total == 0) {
            return Err(Error::InvalidInput(
                "offset is beyond the end of the file".into(),
            ));
        }
        Ok(Output::new(
            ReadPage {
                path: path.display().to_string(),
                lines: self.lines,
                truncated: self.next_offset.is_some(),
                next_offset: self.next_offset,
                revision,
            },
            self.text,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    struct Fragmented<'a> {
        bytes: &'a [u8],
        chunk: usize,
    }

    impl AsyncRead for Fragmented<'_> {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let this = self.get_mut();
            assert!(buffer.remaining() <= READ_CHUNK_BYTES);
            let count = this.chunk.min(this.bytes.len()).min(buffer.remaining());
            let (chunk, remaining) = this.bytes.split_at(count);
            buffer.put_slice(chunk);
            this.bytes = remaining;
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn fragmented_utf8_bom_and_line_endings_match_standard_text_lines() {
        for text in [
            "",
            "\u{feff}",
            "\u{feff}π🦀\r\n\nlast\r",
            "\r\n\r\nx\n",
            "x\r",
        ] {
            let source: Vec<_> = text
                .strip_prefix('\u{feff}')
                .unwrap_or(text)
                .lines()
                .collect();
            for chunk in 1..=8 {
                for offset in 1..=source.len().max(1) {
                    let mut input = ReadInput::new("fragmented");
                    input.offset = NonZeroUsize::new(offset).unwrap();
                    input.limit = NonZeroUsize::new(2);
                    let result = scan(
                        Fragmented {
                            bytes: text.as_bytes(),
                            chunk,
                        },
                        Path::new("fragmented"),
                        &input,
                        ReadLimits::default(),
                    )
                    .await
                    .unwrap();
                    let expected: Vec<_> = source
                        .iter()
                        .enumerate()
                        .skip(offset - 1)
                        .take(2)
                        .map(|(i, text)| ReadLine {
                            number: i + 1,
                            text: (*text).to_owned(),
                        })
                        .collect();
                    assert_eq!(
                        result.data.lines, expected,
                        "chunk={chunk}, offset={offset}"
                    );
                    assert_eq!(
                        result.data.next_offset,
                        (offset + 1 < source.len()).then_some(offset + 2)
                    );
                    assert_eq!(result.data.revision, crate::revision::of(text.as_bytes()));
                }
            }
        }
    }

    #[tokio::test]
    async fn invalid_bytes_after_a_full_page_are_still_refused() {
        for bytes in [b"valid\n\xff".as_slice(), b"valid\n\xf0\x9f", b"valid\n\0"] {
            for chunk in 1..=8 {
                let mut input = ReadInput::new("fragmented");
                input.limit = NonZeroUsize::new(1);
                assert!(matches!(
                    scan(
                        Fragmented { bytes, chunk },
                        Path::new("fragmented"),
                        &input,
                        ReadLimits::default(),
                    )
                    .await,
                    Err(Error::Unsupported(_))
                ));
            }
        }
    }

    #[tokio::test]
    async fn a_scan_refuses_growth_past_the_ingestion_budget() {
        let limits = ReadLimits {
            max_file_bytes: 4,
            ..ReadLimits::default()
        };
        assert!(matches!(
            scan(
                Fragmented {
                    bytes: b"a\nb\nc\n",
                    chunk: 2
                },
                Path::new("growing"),
                &ReadInput::new("growing"),
                limits,
            )
            .await,
            Err(Error::LimitExceeded(_))
        ));
    }

    #[tokio::test]
    async fn ready_readers_yield_so_an_in_progress_scan_can_be_cancelled() {
        let mut reader = Fragmented {
            bytes: b"a\nb\nc\n",
            chunk: 2,
        };
        let input = ReadInput::new("fragmented");
        let cancel = CancellationToken::new();
        {
            let scan = scan(
                &mut reader,
                Path::new("fragmented"),
                &input,
                ReadLimits::default(),
            );
            tokio::pin!(scan);
            // One poll consumes a chunk and reaches the cooperative yield.
            std::future::poll_fn(|ctx| {
                assert!(scan.as_mut().poll(ctx).is_pending());
                Poll::Ready(())
            })
            .await;
            cancel.cancel();
            let result = tokio::select! { biased;
                () = cancel.cancelled() => Err(Error::Cancelled),
                result = scan => result,
            };
            assert!(matches!(result, Err(Error::Cancelled)));
        }
        assert_eq!(reader.bytes, b"b\nc\n");
    }
}
