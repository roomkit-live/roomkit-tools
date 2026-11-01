use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::ReadLine;
use crate::paths::{self, PathEntry, PathKind, Root, Scan, first_entry, workspace_path};

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
    /// Complete source line without its line ending.
    pub text: String,
    /// Complete preceding source lines. Nearby matches may share context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<ReadLine>,
    /// Complete following source lines. Context does not advance the match offset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<ReadLine>,
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
        }
    }
}

/// Bounded, cancellable content search; child symlinks are never followed.
///
/// Hidden files are included and ignore files are not consulted. Binary and
/// non-UTF-8 files are skipped and counted. Other read failures and ingestion
/// limits fail the call. A fresh scan is made for every page.
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
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: GrepLimits::default(),
        })
    }

    /// Set positive host-owned bounds.
    ///
    /// # Errors
    /// Returns invalid input for zero bounds or an unrepresentable read limit.
    pub fn with_limits(mut self, limits: GrepLimits) -> Result<Self, Error> {
        if [
            limits.max_matches,
            limits.max_output_bytes,
            limits.max_scan_entries,
            limits.max_depth,
            limits.max_file_bytes,
            limits.max_total_bytes,
        ]
        .contains(&0)
            || u64::try_from(limits.max_file_bytes).map_or(true, |n| n == u64::MAX)
        {
            return Err(Error::InvalidInput(
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
            "Search UTF-8 file contents by regex or literal text, with an optional file glob. Returns complete matching lines, paths and line numbers. Optional context_before/context_after add surrounding source lines to each match (host-bounded, default 0). Hidden files are included, child symlinks are not followed, and binary files are skipped and counted. Match offsets count matches only, starting at 1; continue with next_offset.",
        )
    }

    /// Search one page of matching lines; cancellation stops waiting for work.
    ///
    /// # Errors
    /// Returns invalid patterns/offsets, refusal outside the root, I/O errors,
    /// unsupported entries or names, or traversal/ingestion/output limit failures.
    pub async fn grep(
        &self,
        input: GrepInput,
        cancel: &CancellationToken,
    ) -> Result<Output<GrepPage>, Error> {
        tokio::select! { biased;
            () = cancel.cancelled() => Err(Error::Cancelled),
            result = self.search(&input) => result,
        }
    }

    /// Files to search and the prefix that `include` patterns are relative to.
    /// `include_depth` stops the walk where the include glob cannot match.
    async fn files(
        &self,
        path: &Path,
        include_depth: Option<usize>,
    ) -> Result<(Scan, String), Error> {
        let info = fs::metadata(path).await.map_err(|source| Error::Io {
            operation: "inspect the search path",
            source,
        })?;
        if info.is_file() {
            let relative = self.root.relative(path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Error::Unsupported("filename is not UTF-8".into()))?
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

    async fn search(&self, input: &GrepInput) -> Result<Output<GrepPage>, Error> {
        if input.context_before > self.limits.max_context_lines
            || input.context_after > self.limits.max_context_lines
        {
            return Err(Error::LimitExceeded(
                "requested grep context exceeds max_context_lines".into(),
            ));
        }
        if input.pattern.is_empty() || input.pattern.len() > 4096 {
            return Err(Error::InvalidInput(
                "grep pattern must contain 1 to 4096 bytes".into(),
            ));
        }
        let expression = if input.literal {
            regex::escape(&input.pattern)
        } else {
            input.pattern.clone()
        };
        let regex = RegexBuilder::new(&expression)
            .case_insensitive(input.ignore_case)
            .size_limit(1024 * 1024)
            .build()
            .map_err(|error| Error::InvalidInput(format!("invalid regex: {error}")))?;
        let include = input.include.as_deref().map(paths::pattern).transpose()?;
        let path = self.root.resolve(&input.path).await?;
        let include_depth = input.include.as_deref().and_then(paths::match_depth);
        let (files, prefix) = self.files(&path, include_depth).await?;
        let mut page = Matches::new(self.root.relative(&path)?, input, self.limits);
        page.data.skipped_entries = files.skipped;
        let mut ingested = 0;
        'files: for file in files
            .entries
            .into_iter()
            .filter(|file| file.kind == PathKind::File)
        {
            let relative = file.path.strip_prefix(&prefix).unwrap_or(&file.path);
            if include
                .as_ref()
                .is_some_and(|pattern| !pattern.is_match(relative))
            {
                continue;
            }
            let resolved = self.root.resolve(&file.path).await?;
            let remaining = self.limits.max_total_bytes - ingested;
            let bytes = read_bounded(resolved, self.limits.max_file_bytes.min(remaining)).await?;
            ingested += bytes.len();
            let source = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
            let Ok(source) = std::str::from_utf8(source) else {
                page.data.skipped_files += 1;
                continue;
            };
            if source.contains('\0') {
                page.data.skipped_files += 1;
                continue;
            }
            let mut preceding = VecDeque::new();
            let mut lines = source.lines().enumerate();
            while let Some((index, line)) = lines.next() {
                if index % 256 == 0 {
                    tokio::task::yield_now().await;
                }
                if regex.is_match(line)
                    && !page.push(
                        &file.path,
                        index + 1,
                        line,
                        &preceding,
                        lines
                            .clone()
                            .take(input.context_after)
                            .map(|(i, text)| (i + 1, text)),
                    )?
                {
                    break 'files;
                }
                if input.context_before > 0 {
                    if preceding.len() == input.context_before {
                        preceding.pop_front();
                    }
                    preceding.push_back((index + 1, line));
                }
            }
        }
        page.finish()
    }
}

async fn read_bounded(path: PathBuf, maximum: usize) -> Result<Vec<u8>, Error> {
    let info = fs::metadata(&path).await.map_err(|source| Error::Io {
        operation: "inspect the search file",
        source,
    })?;
    if !info.is_file() {
        return Err(Error::Unsupported("grep accepts regular files only".into()));
    }
    let maximum = u64::try_from(maximum)
        .map_err(|_| Error::InvalidInput("ingestion limit is too large".into()))?;
    if info.len() > maximum {
        return Err(Error::LimitExceeded(
            "grep exceeds max_file_bytes or max_total_bytes".into(),
        ));
    }
    let file = File::open(path).await.map_err(|source| Error::Io {
        operation: "open the search file",
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| Error::Io {
            operation: "read the search file",
            source,
        })?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > maximum) {
        return Err(Error::LimitExceeded(
            "grep exceeds max_file_bytes or max_total_bytes".into(),
        ));
    }
    Ok(bytes)
}

struct Matches {
    data: GrepPage,
    text: String,
    seen: usize,
    offset: usize,
    limit: usize,
    bytes: usize,
}

impl Matches {
    fn new(path: String, input: &GrepInput, limits: GrepLimits) -> Self {
        Self {
            data: GrepPage {
                path,
                matches: Vec::new(),
                truncated: false,
                next_offset: None,
                skipped_files: 0,
                skipped_entries: 0,
            },
            text: String::new(),
            seen: 0,
            offset: input.offset.get(),
            limit: input
                .limit
                .map_or(limits.max_matches, NonZeroUsize::get)
                .min(limits.max_matches),
            bytes: limits.max_output_bytes,
        }
    }
    fn push<'a>(
        &mut self,
        path: &str,
        line: usize,
        text: &str,
        before: &VecDeque<(usize, &str)>,
        after: impl Iterator<Item = (usize, &'a str)>,
    ) -> Result<bool, Error> {
        self.seen += 1;
        if self.seen < self.offset {
            return Ok(true);
        }
        if self.data.matches.len() >= self.limit {
            return self.end_page();
        }
        let mut rendered = String::new();
        let mut context_before = Vec::new();
        let mut context_after = Vec::new();
        let budget = self.bytes - self.text.len();
        for &(number, source) in before {
            if !render_line(&mut rendered, path, number, source, false, budget) {
                return self.end_page();
            }
            context_before.push(ReadLine {
                number,
                text: source.to_owned(),
            });
        }
        if !render_line(&mut rendered, path, line, text, true, budget) {
            return self.end_page();
        }
        for (number, source) in after {
            if !render_line(&mut rendered, path, number, source, false, budget) {
                return self.end_page();
            }
            context_after.push(ReadLine {
                number,
                text: source.to_owned(),
            });
        }
        self.text.push_str(&rendered);
        self.data.matches.push(GrepMatch {
            path: path.to_owned(),
            line,
            text: text.to_owned(),
            context_before,
            context_after,
        });
        Ok(true)
    }
    fn end_page(&mut self) -> Result<bool, Error> {
        if self.data.matches.is_empty() {
            return Err(Error::LimitExceeded(
                "one matching line and its context exceed max_output_bytes".into(),
            ));
        }
        self.data.truncated = true;
        self.data.next_offset = Some(self.seen);
        Ok(false)
    }
    fn finish(self) -> Result<Output<GrepPage>, Error> {
        if self.offset > self.seen && !(self.offset == 1 && self.seen == 0) {
            return Err(Error::InvalidInput(
                "offset is beyond the last match".into(),
            ));
        }
        Ok(Output::new(self.data, self.text))
    }
}

fn render_line(
    output: &mut String,
    path: &str,
    number: usize,
    text: &str,
    matched: bool,
    budget: usize,
) -> bool {
    let prefix = if matched {
        format!("{path:?}:{number}: ")
    } else {
        format!("{path:?}-{number}- ")
    };
    if prefix.len().saturating_add(text.len()).saturating_add(1) > budget - output.len() {
        return false;
    }
    output.push_str(&prefix);
    output.push_str(text);
    output.push('\n');
    true
}
