use std::path::Path;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::diff_render::{self, Bound, Labels, Line, Rendered};
use crate::{diff_align, mutation, paths::Root};

/// Two files to compare.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffInput {
    /// File with the old version.
    pub old_path: String,
    /// File with the new version.
    pub new_path: String,
    /// Unchanged lines around each change, capped by the host.
    #[serde(default = "default_context")]
    pub context_lines: usize,
}

const fn default_context() -> usize {
    3
}

impl DiffInput {
    /// Compare two files with three lines of context.
    pub fn new(old_path: impl Into<String>, new_path: impl Into<String>) -> Self {
        Self {
            old_path: old_path.into(),
            new_path: new_path.into(),
            context_lines: default_context(),
        }
    }
}

/// Host-owned input, algorithm and presentation bounds.
#[derive(Clone, Copy, Debug)]
pub struct DiffLimits {
    /// Maximum bytes per file. Default: 8 MiB.
    pub max_file_bytes: usize,
    /// Maximum LF-delimited lines per file. Default: 20,000.
    pub max_lines: usize,
    /// Maximum work of the line comparison, a Myers diff: once the common
    /// first and last lines are set aside, each remaining line examined, each
    /// furthest-reaching path computed and each pair of equal lines followed.
    /// It grows with the number of changed lines, about their count squared,
    /// not with the files' length; memory follows it. Default: 1,000,000.
    pub max_comparison_cells: usize,
    /// Maximum rendered diff bytes; exceeding this fails without clipping. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum context lines on each side of a change. Default: 100.
    pub max_context_lines: usize,
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_lines: 20_000,
            max_comparison_cells: 1_000_000,
            max_output_bytes: 50 * 1024,
            max_context_lines: 100,
        }
    }
}

impl DiffLimits {
    pub(crate) fn validate(self) -> Result<(), ToolError> {
        mutation::validate_limit(self.max_file_bytes)?;
        if self.max_lines == 0
            || self.max_lines > u32::MAX as usize / 2
            || self.max_comparison_cells == 0
            || self.max_output_bytes == 0
        {
            return Err(ToolError::InvalidInput(
                "diff bounds must be positive and representable".into(),
            ));
        }
        Ok(())
    }
}

/// One line's role in a diff.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    /// Present unchanged in both versions.
    Context,
    /// Present only in the old version.
    Removed,
    /// Present only in the new version.
    Added,
}

/// A source line, including its original line ending when present.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiffLine {
    /// Whether the line is unchanged, removed or added.
    pub kind: DiffLineKind,
    /// One-based old line number, absent for additions.
    pub old_line: Option<usize>,
    /// One-based new line number, absent for removals.
    pub new_line: Option<usize>,
    /// Exact text, including BOM and LF/CRLF bytes. No normalization is performed.
    pub text: String,
}

/// A group of adjacent changes with bounded surrounding context.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiffHunk {
    /// One-based first old line, or preceding line number for an empty range.
    pub old_start: usize,
    /// Number of old lines in this hunk.
    pub old_count: usize,
    /// One-based first new line, or preceding line number for an empty range.
    pub new_start: usize,
    /// Number of new lines in this hunk.
    pub new_count: usize,
    /// Source lines in diff order.
    pub lines: Vec<DiffLine>,
}

/// A complete comparison; oversized results fail instead of silently truncating.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiffResult {
    /// Old version's workspace-relative label.
    pub old_path: String,
    /// New version's workspace-relative label.
    pub new_path: String,
    /// Whether bytes differ.
    pub changed: bool,
    /// Total added lines.
    pub additions: usize,
    /// Total removed lines.
    pub deletions: usize,
    /// Actual context bound used for this comparison.
    pub context_lines: usize,
    /// Hunks, with exact source text and line numbers; complete, unless a
    /// patch preview was clipped to its output bound.
    pub hunks: Vec<DiffHunk>,
}

/// Independently selectable, read-only file comparison.
#[derive(Clone, Debug)]
pub struct DiffTool {
    root: Root,
    limits: DiffLimits,
}

impl DiffTool {
    /// Select an existing workspace root.
    ///
    /// # Errors
    /// Returns root resolution errors or invalid input for a non-directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: DiffLimits::default(),
        })
    }

    /// Replace host-owned bounds.
    ///
    /// # Errors
    /// Rejects zero or unrepresentable resource bounds. Zero context is supported.
    pub fn with_limits(mut self, limits: DiffLimits) -> Result<Self, ToolError> {
        limits.validate()?;
        self.limits = limits;
        Ok(self)
    }

    /// Derived schemas for file comparison.
    pub fn definition() -> Definition {
        Definition::new::<DiffInput, DiffResult>(
            "diff",
            "Compare two existing UTF-8 files without changing them: hunks, added/removed counts and a unified preview. Line endings and BOM count. No Git and no paging: an oversized comparison fails.",
        )
    }

    /// Read two files and compare them on a blocking worker.
    ///
    /// # Errors
    /// Returns path refusal, unsupported text, I/O, cancellation or bounds errors.
    /// The two reads are not a transaction against external editors.
    pub async fn diff(
        &self,
        input: DiffInput,
        cancel: &CancellationToken,
    ) -> Result<Output<DiffResult>, ToolError> {
        let token = cancel.child_token();
        let _cancel_on_drop = token.clone().drop_guard();
        let compared = tokio::select! { biased;
            () = token.cancelled() => Err(ToolError::Cancelled),
            result = self.compare_files(input, token.clone()) => result,
        }?;
        // Equal files have no diff to show; a model still reads that it ran.
        Ok(if compared.data.changed {
            compared
        } else {
            Output::new(compared.data, "no differences\n")
        })
    }

    async fn compare_files(
        &self,
        input: DiffInput,
        cancel: CancellationToken,
    ) -> Result<Output<DiffResult>, ToolError> {
        let (old, new) = tokio::try_join!(self.load(&input.old_path), self.load(&input.new_path))?;
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            compare(
                &old.1,
                &new.1,
                &old.0,
                &new.0,
                input.context_lines,
                limits,
                &cancel,
            )
        })
        .await
        .map_err(|error| mutation::io("finish the comparison", std::io::Error::other(error)))?
    }

    async fn load(&self, input: &str) -> Result<(String, String), ToolError> {
        let path = self.root.resolve(input).await?;
        let info = tokio::fs::metadata(&path)
            .await
            .map_err(|error| mutation::io("inspect diff file", error))?;
        if !info.is_file() {
            return Err(ToolError::UnsupportedResource(
                "diff accepts regular files only".into(),
            ));
        }
        if info.len() > self.limits.max_file_bytes as u64 {
            return Err(ToolError::LimitExceeded(
                "diff file exceeds max_file_bytes".into(),
            ));
        }
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|error| mutation::io("open diff file", error))?;
        let mut bytes = Vec::new();
        file.take(self.limits.max_file_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| mutation::io("read diff file", error))?;
        if bytes.len() > self.limits.max_file_bytes {
            return Err(ToolError::LimitExceeded(
                "diff file exceeds max_file_bytes".into(),
            ));
        }
        let text = mutation::validate_text(&bytes)?.to_owned();
        Ok((self.root.relative(&path)?, text))
    }
}

/// Compare two texts for the diff tool: a comparison over a host bound is
/// refused.
pub(crate) fn compare(
    old: &str,
    new: &str,
    old_path: &str,
    new_path: &str,
    context: usize,
    limits: DiffLimits,
    cancel: &CancellationToken,
) -> Result<Output<DiffResult>, ToolError> {
    let kinds = |old: &[&str], new: &[&str]| {
        diff_align::align(old, new, limits.max_comparison_cells, cancel)?.ok_or_else(|| {
            ToolError::LimitExceeded(
                "diff exceeds max_comparison_cells: too many lines differ to compare within the host's bound; compare smaller or closer files".into(),
            )
        })
    };
    let lines = aligned(old, new, limits, cancel, kinds)?;
    let labels = Labels {
        old: old_path,
        new: new_path,
    };
    let context = context.min(limits.max_context_lines);
    let rendered = diff_render::render(
        &lines,
        &labels,
        context,
        limits.max_output_bytes,
        Bound::Refuse,
        cancel,
    )?;
    Ok(Output::new(rendered.data, rendered.text))
}

/// Compare two texts for a patch's preview, which is never refused for its
/// size: past `max_comparison_cells` the lines between the common ends are
/// shown replaced whole, and past `budget` bytes the preview is clipped.
pub(crate) fn preview(
    old: &str,
    new: &str,
    labels: &Labels<'_>,
    limits: DiffLimits,
    budget: usize,
    cancel: &CancellationToken,
) -> Result<Rendered, ToolError> {
    let kinds = |old: &[&str], new: &[&str]| {
        Ok(
            diff_align::align(old, new, limits.max_comparison_cells, cancel)?
                .unwrap_or_else(|| diff_align::replace_middle(old, new)),
        )
    };
    let lines = aligned(old, new, limits, cancel, kinds)?;
    let context = PREVIEW_CONTEXT.min(limits.max_context_lines);
    diff_render::render(&lines, labels, context, budget, Bound::Clip, cancel)
}

/// Context lines around a change in a patch's preview.
const PREVIEW_CONTEXT: usize = 3;

/// The lines of both texts, aligned by `kinds`, within the file bounds.
fn aligned<'a>(
    old: &'a str,
    new: &'a str,
    limits: DiffLimits,
    cancel: &CancellationToken,
    kinds: impl FnOnce(&[&str], &[&str]) -> Result<Vec<DiffLineKind>, ToolError>,
) -> Result<Vec<Line<'a>>, ToolError> {
    mutation::bounded_text(old, limits.max_file_bytes)?;
    mutation::bounded_text(new, limits.max_file_bytes)?;
    mutation::check_cancel(cancel)?;
    let split = |text: &'a str| -> Vec<&'a str> {
        text.split_inclusive('\n')
            .take(limits.max_lines + 1)
            .collect()
    };
    let (old_lines, new_lines) = (split(old), split(new));
    if old_lines.len().max(new_lines.len()) > limits.max_lines {
        return Err(ToolError::LimitExceeded("diff exceeds max_lines".into()));
    }
    let kinds = kinds(&old_lines, &new_lines)?;
    let mut lines = Vec::with_capacity(kinds.len());
    let (mut old_iter, mut new_iter) = (old_lines.into_iter(), new_lines.into_iter());
    let (mut old_number, mut new_number) = (0, 0);
    for kind in kinds {
        let value = match kind {
            DiffLineKind::Context => {
                new_iter.next();
                old_iter.next()
            }
            DiffLineKind::Removed => old_iter.next(),
            DiffLineKind::Added => new_iter.next(),
        }
        .ok_or_else(|| ToolError::InvalidInput("invalid diff alignment".into()))?;
        lines.push(Line {
            kind,
            value,
            old: old_number,
            new: new_number,
        });
        old_number += usize::from(kind != DiffLineKind::Added);
        new_number += usize::from(kind != DiffLineKind::Removed);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_context_reconstructs_both_inputs_even_with_repeated_lines() {
        let mut versions = vec![String::new()];
        for length in 1..=5 {
            for bits in 0..(1 << length) {
                versions.push(
                    (0..length)
                        .map(|bit| {
                            if bits & (1 << bit) == 0 {
                                "a\n"
                            } else {
                                "β\n"
                            }
                        })
                        .collect(),
                );
            }
        }
        for old in &versions {
            for new in &versions {
                let result = compare(
                    old,
                    new,
                    "old",
                    "new",
                    100,
                    DiffLimits::default(),
                    &CancellationToken::new(),
                )
                .unwrap();
                if old == new {
                    assert!(!result.data.changed);
                    continue;
                }
                let lines: Vec<_> = result
                    .data
                    .hunks
                    .iter()
                    .flat_map(|hunk| &hunk.lines)
                    .collect();
                let restored_old: String = lines
                    .iter()
                    .filter(|line| line.kind != DiffLineKind::Added)
                    .map(|line| line.text.as_str())
                    .collect();
                let restored_new: String = lines
                    .iter()
                    .filter(|line| line.kind != DiffLineKind::Removed)
                    .map(|line| line.text.as_str())
                    .collect();
                assert_eq!(&restored_old, old);
                assert_eq!(&restored_new, new);
            }
        }
    }
}
