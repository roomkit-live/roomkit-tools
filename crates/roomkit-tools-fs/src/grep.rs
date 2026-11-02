use std::num::NonZeroUsize;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::ReadLine;
use crate::grep_scan::{self, Matches, Search};
use crate::paths::{self, PathEntry, PathKind, Root, Scan, first_entry, workspace_path};
use crate::worker;

/// Search UTF-8 file contents, one match per matching line.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrepInput {
    /// Text or Rust-regex expression to search for. Multiline matches are not supported.
    pub pattern: String,
    /// File or directory beneath the workspace root. Defaults to `.`.
    #[serde(default = "workspace_path")]
    pub path: String,
    /// Optional glob for file paths relative to the search directory.
    #[serde(default)]
    pub include: Option<String>,
    /// Match the pattern as literal text instead of a regex. Defaults to false.
    #[serde(default)]
    pub literal: bool,
    /// Ignore Unicode letter case. Defaults to false.
    #[serde(default)]
    pub ignore_case: bool,
    /// Lines of context before each match. Defaults to zero; bounded by the host.
    #[serde(default)]
    pub context_before: usize,
    /// Lines of context after each match. Defaults to zero; bounded by the host.
    #[serde(default)]
    pub context_after: usize,
    /// One-based match offset, ordered by path then line.
    #[serde(default = "first_entry")]
    pub offset: NonZeroUsize,
    /// Requested matching lines, capped by the host.
    #[serde(default)]
    pub limit: Option<NonZeroUsize>,
}

impl GrepInput {
    /// Search the workspace root with a case-sensitive regex.
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            path: workspace_path(),
            include: None,
            literal: false,
            ignore_case: false,
            context_before: 0,
            context_after: 0,
            offset: first_entry(),
            limit: None,
        }
    }
}

/// One matching line of text.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GrepMatch {
    /// Workspace-relative file path, usable by `read`.
    pub path: String,
    /// One-based source line number.
    pub line: usize,
    /// Source line without its line ending; complete unless `clipped`.
    pub text: String,
    /// Complete preceding source lines. Nearby matches may share context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<ReadLine>,
    /// Complete following source lines. Context does not advance the match offset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<ReadLine>,
    /// True when this match alone exceeded `max_output_bytes`: its context is
    /// omitted and `text` is cut at a UTF-8 boundary to fit. Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clipped: bool,
}

/// Matching lines and continuation information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GrepPage {
    /// Workspace-relative file or directory searched; `.` is the root.
    pub path: String,
    /// Matching lines, ordered by path then line number.
    pub matches: Vec<GrepMatch>,
    /// Whether another matching line exists.
    pub truncated: bool,
    /// One-based match offset for the next page, or `None` at the end.
    pub next_offset: Option<usize>,
    /// Files skipped as binary or non-UTF-8 before this page ended.
    pub skipped_files: usize,
    /// Entries left out of the whole scan because their names are not UTF-8;
    /// a skipped directory is not searched.
    pub skipped_entries: usize,
    /// Lines longer than `max_line_bytes`, not searched, before this page ended.
    pub skipped_lines: usize,
}

/// Host-owned traversal, ingestion and output bounds.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct GrepLimits {
    /// Maximum matching lines per page. Default: 200.
    pub max_matches: usize,
    /// Maximum rendered bytes. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum directory entries visited, including nonmatches. Default: 10,000.
    pub max_scan_entries: usize,
    /// Maximum directory levels scanned; starting directory is level one. Default: 32.
    pub max_depth: usize,
    /// Maximum bytes ingested from one file. Default: 8 MiB.
    pub max_file_bytes: usize,
    /// Maximum bytes ingested across all scanned files in one call. Default: 32 MiB.
    pub max_total_bytes: usize,
    /// Maximum requested context lines on either side of a match. Default: 20.
    /// Zero disables context without disabling ordinary matching.
    pub max_context_lines: usize,
    /// Longest line handed to the regex, in bytes. Longer lines are not
    /// searched and are counted in `skipped_lines`; this bounds the work done
    /// between cancellation checks. Default: 64 KiB.
    pub max_line_bytes: usize,
}

impl Default for GrepLimits {
    fn default() -> Self {
        Self {
            max_matches: 200,
            max_output_bytes: 50 * 1024,
            max_scan_entries: 10_000,
            max_depth: 32,
            max_file_bytes: 8 * 1024 * 1024,
            max_total_bytes: 32 * 1024 * 1024,
            max_context_lines: 20,
            max_line_bytes: 64 * 1024,
        }
    }
}

/// Bounded, cancellable content search; child symlinks are never followed.
///
/// Hidden files are included and ignore files are not consulted. Binary and
/// non-UTF-8 files are skipped and counted. Other read failures and ingestion
/// limits fail the call. A fresh scan is made for every page. Reading and
/// matching run on a blocking worker, never on the async executor.
#[derive(Clone, Debug)]
pub struct GrepTool {
    root: Root,
    limits: GrepLimits,
}

impl GrepTool {
    /// Construct a tool beneath an existing directory.
    ///
    /// # Errors
    /// Returns an I/O error for an unresolved root, or invalid input for a file.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: GrepLimits::default(),
        })
    }

    /// Set positive host-owned bounds.
    ///
    /// # Errors
    /// Returns invalid input for zero bounds or an unrepresentable read limit.
    pub fn with_limits(mut self, limits: GrepLimits) -> Result<Self, ToolError> {
        if [
            limits.max_matches,
            limits.max_output_bytes,
            limits.max_scan_entries,
            limits.max_depth,
            limits.max_file_bytes,
            limits.max_total_bytes,
            limits.max_line_bytes,
        ]
        .contains(&0)
            || u64::try_from(limits.max_file_bytes).map_or(true, |n| n == u64::MAX)
        {
            return Err(ToolError::InvalidInput(
                "grep limits must be positive and representable".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// The definition derived from actual input and output types.
    pub fn definition() -> Definition {
        Definition::new::<GrepInput, GrepPage>(
            "grep",
            "Search UTF-8 file contents by regex or literal text, with an optional file glob. Returns matching lines, paths and line numbers. Optional context_before/context_after add surrounding source lines to each match (host-bounded, default 0). A match too large for the output is returned alone, without context and with its text cut, as clipped: true. Lines longer than the host's line bound are not searched and are counted in skipped_lines. Hidden files are included, child symlinks are not followed, and binary files are skipped and counted. Match offsets count matches only, starting at 1; continue with next_offset.",
        )
    }

    /// Search one page of matching lines.
    ///
    /// Cancellation, or dropping the returned future, returns promptly; the
    /// blocking search worker stops at its next check, before the next file
    /// or line, so its remaining work is at most one `max_line_bytes` line.
    ///
    /// # Errors
    /// Returns invalid patterns/offsets, refusal outside the root, I/O errors,
    /// unsupported entries, a budget too small for one match reference, or
    /// traversal/ingestion limit failures.
    pub async fn grep(
        &self,
        input: GrepInput,
        cancel: &CancellationToken,
    ) -> Result<Output<GrepPage>, ToolError> {
        worker::cancellable(cancel, |cancel| self.search(input, cancel)).await
    }

    /// Files to search and the prefix that `include` patterns are relative to.
    /// `include_depth` stops the walk where the include glob cannot match.
    async fn files(
        &self,
        path: &Path,
        include_depth: Option<usize>,
    ) -> Result<(Scan, String), ToolError> {
        let info = fs::metadata(path).await.map_err(|source| ToolError::Io {
            operation: "inspect the search path",
            source,
        })?;
        if info.is_file() {
            let relative = self.root.relative(path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| ToolError::UnsupportedResource("filename is not UTF-8".into()))?
                .to_owned();
            let prefix = relative.strip_suffix(&name).unwrap_or_default().to_owned();
            let entries = vec![PathEntry {
                path: relative,
                name,
                kind: PathKind::File,
            }];
            return Ok((
                Scan {
                    entries,
                    skipped: 0,
                },
                prefix,
            ));
        }
        let prefix = self.root.prefix(path)?;
        let scan = paths::walk(
            &self.root,
            path,
            self.limits.max_scan_entries,
            self.limits.max_depth,
            include_depth,
        )
        .await?;
        Ok((scan, prefix))
    }

    /// Validate and walk on the async side, then read and match on a
    /// blocking worker that observes `cancel`.
    async fn search(
        &self,
        input: GrepInput,
        cancel: CancellationToken,
    ) -> Result<Output<GrepPage>, ToolError> {
        let regex = self.regex(&input)?;
        let include = input.include.as_deref().map(paths::pattern).transpose()?;
        let path = self.root.resolve(&input.path).await?;
        let include_depth = input.include.as_deref().and_then(paths::match_depth);
        let (files, prefix) = self.files(&path, include_depth).await?;
        let mut page = Matches::new(self.root.relative(&path)?, &input, self.limits);
        page.data.skipped_entries = files.skipped;
        let search = Search {
            root: self.root.clone(),
            files: files.entries,
            prefix,
            include,
            regex,
            input,
            limits: self.limits,
            page,
        };
        worker::blocking("finish the search task", move || {
            grep_scan::run(search, &cancel)
        })
        .await
    }

    /// Check model arguments against host bounds and compile the pattern.
    fn regex(&self, input: &GrepInput) -> Result<Regex, ToolError> {
        if input.context_before > self.limits.max_context_lines
            || input.context_after > self.limits.max_context_lines
        {
            return Err(ToolError::LimitExceeded(
                "requested grep context exceeds max_context_lines".into(),
            ));
        }
        if input.pattern.is_empty() || input.pattern.len() > 4096 {
            return Err(ToolError::InvalidInput(
                "grep pattern must contain 1 to 4096 bytes".into(),
            ));
        }
        let expression = if input.literal {
            regex::escape(&input.pattern)
        } else {
            input.pattern.clone()
        };
        RegexBuilder::new(&expression)
            .case_insensitive(input.ignore_case)
            .size_limit(1024 * 1024)
            .build()
            .map_err(|error| ToolError::InvalidInput(format!("invalid regex: {error}")))
    }
}
