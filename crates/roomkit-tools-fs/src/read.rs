use std::num::NonZeroUsize;
use std::path::Path;

use crate::paths::Root;
use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

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
    /// Maximum bytes ingested from one file. Default: 8 MiB.
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
            "Read a page of UTF-8 text from a file inside the configured workspace. Lines are numbered from one. Large pages return next_offset; binary files and files above the host's ingestion limit are refused.",
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
        let mut bytes = Vec::new();
        file.take(maximum + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| Error::Io {
                operation: "read the file",
                source,
            })?;
        if bytes.len() > self.limits.max_file_bytes {
            return Err(Error::LimitExceeded("file exceeds max_file_bytes".into()));
        }
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::Unsupported("file is not UTF-8 text".into()))?;
        if text.contains('\0') {
            return Err(Error::Unsupported("file contains binary NUL bytes".into()));
        }
        render_page(&path, text, &input, self.limits)
    }
}

fn render_page(
    path: &Path,
    text: &str,
    input: &ReadInput,
    limits: ReadLimits,
) -> Result<Output<ReadPage>, Error> {
    let offset = input.offset.get();
    let total = text.lines().count();
    if offset > total && !(offset == 1 && total == 0) {
        return Err(Error::InvalidInput(
            "offset is beyond the end of the file".into(),
        ));
    }
    let limit = input
        .limit
        .map_or(limits.max_lines, NonZeroUsize::get)
        .min(limits.max_lines);
    let mut lines = Vec::new();
    let mut rendered = String::new();
    let mut next_offset = None;
    for (index, line) in text.lines().enumerate().skip(offset - 1) {
        let number = index + 1;
        let numbered = format!("{number}: {line}\n");
        if lines.len() >= limit || numbered.len() > limits.max_output_bytes - rendered.len() {
            if lines.is_empty() {
                return Err(Error::LimitExceeded(
                    "one line exceeds max_output_bytes".into(),
                ));
            }
            next_offset = Some(number);
            break;
        }
        rendered.push_str(&numbered);
        lines.push(ReadLine {
            number,
            text: line.to_owned(),
        });
    }
    Ok(Output::new(
        ReadPage {
            path: path.display().to_string(),
            lines,
            truncated: next_offset.is_some(),
            next_offset,
        },
        rendered,
    ))
}
