use std::num::NonZeroUsize;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::ReadLine;
use crate::grep_include::Include;
use crate::grep_scan::{self, Matches, Search};
use crate::ignores::Ignores;
use crate::paths::{PathEntry, PathKind, Root, first_entry, workspace_path};
use crate::walk::{self, Bounds, Scan, Start};
use crate::worker;

/// A search of file contents.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrepInput {
    /// Rust regex, or plain text with `literal`. Single-line matches only.
    pub pattern: String,
    /// File or directory to search.
    #[serde(default = "workspace_path")]
    pub path: String,
    /// Glob on file names at any depth, e.g. `*.rs`; with a `/`, on paths
    /// relative to `path`, e.g. `src/**/*.rs`.
    #[serde(default)]
    pub include: Option<String>,
    /// Also search what `.gitignore` and `.ignore` exclude, and `.git`.
    #[serde(default)]
    pub include_ignored: bool,
    /// Match `pattern` as plain text.
    #[serde(default)]
    pub literal: bool,
    /// Ignore Unicode letter case.
    #[serde(default)]
    pub ignore_case: bool,
    /// Lines shown before each match, capped by the host.
    #[serde(default)]
    pub context_before: usize,
    /// Lines shown after each match, capped by the host; a capped request is
    /// stated in the text.
    #[serde(default)]
    pub context_after: usize,
    /// First match, ordered by path then line.
    #[serde(default = "first_entry")]
    pub offset: NonZeroUsize,
    /// Match count, capped by the host.
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
            include_ignored: false,
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
    /// Whether the search is incomplete: another matching line exists, or a
    /// host bound stopped the search, which the text names (`truncated_by`,
    /// `stopped_at`).
    pub truncated: bool,
    /// One-based match offset for the next page, or `None` at the end or
    /// when a host bound stopped the search.
    pub next_offset: Option<usize>,
    /// Files skipped as binary, non-UTF-8 or larger than `max_file_bytes`
    /// before this page ended; the text names the first.
    pub skipped_files: usize,
    /// Entries left out of the whole scan because their names are not UTF-8;
    /// a skipped directory is not searched.
    pub skipped_entries: usize,
    /// Lines longer than `max_line_bytes`, not searched, before this page ended.
    pub skipped_lines: usize,
}

/// Host-owned traversal, ingestion and output bounds.
#[derive(Clone, Copy, Debug)]
pub struct GrepLimits {
    /// Maximum matching lines per page. Default: 200.
    pub max_matches: usize,
    /// Maximum rendered bytes, the closing line of facts apart. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum directory entries visited, including nonmatches and ignored
    /// entries; directories are visited in path order. Default: 10,000.
    pub max_scan_entries: usize,
    /// Maximum directory levels scanned; starting directory is level one. A
    /// deeper directory is not entered. Default: 32.
    pub max_depth: usize,
    /// Largest file searched, in bytes; a larger one is skipped and counted
    /// in `skipped_files`. Default: 8 MiB.
    pub max_file_bytes: usize,
    /// Maximum bytes read across all files in one call. Once it runs out, the
    /// search returns the matches found so far, truncated, and names the file
    /// it stopped at. Default: 32 MiB.
    pub max_total_bytes: usize,
    /// Maximum context lines on either side of a match: more is capped, and
    /// the text states the context used. Default: 20. Zero disables context
    /// without disabling ordinary matching.
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
/// Hidden files are included. Unless `include_ignored`, what the
/// `.gitignore` and `.ignore` files of the workspace exclude, and `.git`, are
/// not searched; in a Git repository, nor is what its `info/exclude` and the
/// ignore files between its root and the workspace's exclude. Binary and non-UTF-8 files, and files over
/// `max_file_bytes`, are skipped and counted; binary content is told from a
/// file's first 8 KiB, and the rest of it is not read. At a traversal bound,
/// or once `max_total_bytes` runs out, the search returns the matches found
/// so far, truncated. Other read failures fail the call. A fresh scan is made for
/// every page, from the first file.
/// Reading and matching run on a blocking worker, never on the async
/// executor.
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
            "Search UTF-8 file contents, one match per line. Hidden files are searched; what .gitignore or .ignore excludes, and .git, are skipped unless include_ignored; child symlinks are not followed; binary files and files over the host's size bound are skipped and counted (skipped_files), as are lines over its line bound (skipped_lines). clipped: true marks a match cut to fit, shown without context. A host bound reached mid-search returns the matches found, naming where it stopped (truncated_by, stopped_at); narrow path or include to search past it. Continue with next_offset.",
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
    /// an ignore file over its bounds.
    pub async fn grep(
        &self,
        input: GrepInput,
        cancel: &CancellationToken,
    ) -> Result<Output<GrepPage>, ToolError> {
        worker::cancellable(cancel, |cancel| self.search(input, cancel)).await
    }

    /// Files to search and the prefix that `include` patterns are relative to.
    /// The walk starts at the include glob's literal `directories` and stops
    /// at `include_depth`, where it cannot match.
    async fn files(
        &self,
        path: &Path,
        directories: &[&str],
        include_depth: Option<usize>,
        include_ignored: bool,
    ) -> Result<(Scan, String), ToolError> {
        let info = fs::metadata(path).await.map_err(|source| ToolError::Io {
            operation: "inspect the search path",
            source,
        })?;
        if info.is_file() {
            return self.one_file(path);
        }
        let prefix = self.root.prefix(path)?;
        let start = Start {
            ignores: Ignores::for_scan(&self.root, path, include_ignored).await?,
            directory: path.to_path_buf(),
            depth: 1,
        };
        let bounds = Bounds {
            max_scan_entries: self.limits.max_scan_entries,
            max_depth: self.limits.max_depth,
        };
        let scan = match walk::descend(&self.root, start, directories, bounds.max_depth).await? {
            Some(start) => walk::walk(&self.root, start, bounds, include_depth).await?,
            None => Scan::default(),
        };
        Ok((scan, prefix))
    }

    /// The search of `path`, a file: that file alone, `include` relative to
    /// its directory.
    fn one_file(&self, path: &Path) -> Result<(Scan, String), ToolError> {
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
        let scan = Scan {
            entries,
            ..Scan::default()
        };
        Ok((scan, prefix))
    }

    /// Validate and walk on the async side, then read and match on a
    /// blocking worker that observes `cancel`.
    async fn search(
        &self,
        mut input: GrepInput,
        cancel: CancellationToken,
    ) -> Result<Output<GrepPage>, ToolError> {
        let regex = Self::regex(&input)?;
        let asked = (input.context_before, input.context_after);
        input.context_before = asked.0.min(self.limits.max_context_lines);
        input.context_after = asked.1.min(self.limits.max_context_lines);
        let capped = asked != (input.context_before, input.context_after);
        let include = input.include.as_deref().map(Include::new).transpose()?;
        let path = self.root.resolve(&input.path).await?;
        let include_depth = include.as_ref().and_then(Include::depth);
        let directories = match (&include, input.include.as_deref()) {
            (Some(include), Some(pattern)) => include.directories(pattern),
            _ => Vec::new(),
        };
        let (files, prefix) = self
            .files(&path, &directories, include_depth, input.include_ignored)
            .await?;
        let mut page = Matches::new(self.root.relative(&path)?, &input, self.limits);
        if capped {
            page.state_context(&input);
        }
        page.data.skipped_entries = files.skipped;
        if let Some(stop) = files.stopped {
            page.stop(stop);
        }
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

    /// Check the pattern against its bounds and compile it.
    fn regex(input: &GrepInput) -> Result<Regex, ToolError> {
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
