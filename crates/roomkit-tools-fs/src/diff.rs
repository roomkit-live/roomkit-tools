use std::path::Path;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::{mutation, paths::Root};

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
    /// Maximum LCS matrix cells after removing common ends. Default: 1,000,000.
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
    /// Complete hunks, with exact source text and line numbers.
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

struct Line<'a> {
    kind: DiffLineKind,
    value: &'a str,
    old: usize,
    new: usize,
}

pub(crate) fn compare(
    old: &str,
    new: &str,
    old_path: &str,
    new_path: &str,
    context: usize,
    limits: DiffLimits,
    cancel: &CancellationToken,
) -> Result<Output<DiffResult>, ToolError> {
    mutation::bounded_text(old, limits.max_file_bytes)?;
    mutation::bounded_text(new, limits.max_file_bytes)?;
    mutation::check_cancel(cancel)?;
    let old_lines: Vec<_> = old
        .split_inclusive('\n')
        .take(limits.max_lines + 1)
        .collect();
    let new_lines: Vec<_> = new
        .split_inclusive('\n')
        .take(limits.max_lines + 1)
        .collect();
    if old_lines.len().max(new_lines.len()) > limits.max_lines {
        return Err(ToolError::LimitExceeded("diff exceeds max_lines".into()));
    }
    let kinds = align(&old_lines, &new_lines, limits.max_comparison_cells, cancel)?;
    let mut lines = Vec::with_capacity(kinds.len());
    let (mut old_iter, mut new_iter) = (old_lines.into_iter(), new_lines.into_iter());
    let (mut old_number, mut new_number) = (0, 0);
    for kind in kinds {
        mutation::check_cancel(cancel)?;
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
    render(
        &lines,
        old_path,
        new_path,
        context.min(limits.max_context_lines),
        limits.max_output_bytes,
        cancel,
    )
}

// Dimensions are checked before allocation. Every matrix/line index below is
// generated by these dimensions, never by a caller-provided offset.
#[expect(
    clippy::indexing_slicing,
    reason = "matrix and line indices are bounded by checked dimensions"
)]
fn align(
    old: &[&str],
    new: &[&str],
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Vec<DiffLineKind>, ToolError> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let mut output = vec![DiffLineKind::Context; prefix];
    if a.is_empty() || b.is_empty() {
        output.extend(std::iter::repeat_n(DiffLineKind::Removed, a.len()));
        output.extend(std::iter::repeat_n(DiffLineKind::Added, b.len()));
    } else {
        let width = b.len() + 1;
        let size = (a.len() + 1)
            .checked_mul(width)
            .filter(|size| *size <= maximum)
            .ok_or_else(|| {
                ToolError::LimitExceeded(
                    "diff exceeds max_comparison_cells; compare smaller files".into(),
                )
            })?;
        let mut matrix = vec![0_u32; size];
        for i in (0..a.len()).rev() {
            mutation::check_cancel(cancel)?;
            for j in (0..b.len()).rev() {
                if j % 256 == 0 {
                    mutation::check_cancel(cancel)?;
                }
                matrix[i * width + j] = if a[i] == b[j] {
                    matrix[(i + 1) * width + j + 1] + 1
                } else {
                    matrix[(i + 1) * width + j].max(matrix[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            mutation::check_cancel(cancel)?;
            if i < a.len() && j < b.len() && a[i] == b[j] {
                output.push(DiffLineKind::Context);
                i += 1;
                j += 1;
            } else if i < a.len()
                && (j == b.len() || matrix[(i + 1) * width + j] >= matrix[i * width + j + 1])
            {
                output.push(DiffLineKind::Removed);
                i += 1;
            } else {
                output.push(DiffLineKind::Added);
                j += 1;
            }
        }
    }
    output.extend(std::iter::repeat_n(DiffLineKind::Context, suffix));
    Ok(output)
}

fn push(text: &mut String, value: &str, maximum: usize) -> Result<(), ToolError> {
    if value.len() > maximum.saturating_sub(text.len()) {
        return Err(ToolError::LimitExceeded(
            "diff exceeds max_output_bytes".into(),
        ));
    }
    text.push_str(value);
    Ok(())
}

fn render(
    lines: &[Line<'_>],
    old_path: &str,
    new_path: &str,
    context: usize,
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Output<DiffResult>, ToolError> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let (mut additions, mut deletions) = (0, 0);
    for (index, line) in lines.iter().enumerate() {
        match line.kind {
            DiffLineKind::Context => continue,
            DiffLineKind::Added => additions += 1,
            DiffLineKind::Removed => deletions += 1,
        }
        let start = index.saturating_sub(context);
        let end = index
            .saturating_add(context)
            .saturating_add(1)
            .min(lines.len());
        if let Some(previous) = ranges.last_mut().filter(|previous| start <= previous.end) {
            previous.end = end;
        } else {
            ranges.push(start..end);
        }
    }
    let mut text = String::new();
    if !ranges.is_empty() {
        push(
            &mut text,
            &format!("--- {old_path:?}\n+++ {new_path:?}\n"),
            maximum,
        )?;
    }
    let mut hunks = Vec::new();
    for range in ranges {
        mutation::check_cancel(cancel)?;
        let selected = lines
            .get(range)
            .ok_or_else(|| ToolError::InvalidInput("invalid diff range".into()))?;
        let Some(first) = selected.first() else {
            continue;
        };
        let old_count = selected
            .iter()
            .filter(|line| line.kind != DiffLineKind::Added)
            .count();
        let new_count = selected
            .iter()
            .filter(|line| line.kind != DiffLineKind::Removed)
            .count();
        let old_start = first.old + usize::from(old_count != 0);
        let new_start = first.new + usize::from(new_count != 0);
        push(
            &mut text,
            &format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"),
            maximum,
        )?;
        let mut rendered = Vec::new();
        for line in selected {
            mutation::check_cancel(cancel)?;
            push(
                &mut text,
                match line.kind {
                    DiffLineKind::Context => " ",
                    DiffLineKind::Added => "+",
                    DiffLineKind::Removed => "-",
                },
                maximum,
            )?;
            push(&mut text, line.value, maximum)?;
            if !line.value.ends_with('\n') {
                push(&mut text, "\n\\ No newline at end of file\n", maximum)?;
            }
            rendered.push(DiffLine {
                kind: line.kind,
                old_line: (line.kind != DiffLineKind::Added).then_some(line.old + 1),
                new_line: (line.kind != DiffLineKind::Removed).then_some(line.new + 1),
                text: line.value.into(),
            });
        }
        hunks.push(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: rendered,
        });
    }
    Ok(Output::new(
        DiffResult {
            old_path: old_path.into(),
            new_path: new_path.into(),
            changed: additions != 0 || deletions != 0,
            additions,
            deletions,
            context_lines: context,
            hunks,
        },
        text,
    ))
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
