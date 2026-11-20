//! A comparison's hunks and unified-style text, within an output bound that
//! either refuses the comparison or clips it at a line.

use std::ops::Range;

use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

use crate::diff::{DiffHunk, DiffLine, DiffLineKind, DiffResult};
use crate::mutation;

/// One aligned line: its kind, its text, and the old and new lines before it.
pub(crate) struct Line<'a> {
    pub(crate) kind: DiffLineKind,
    pub(crate) value: &'a str,
    pub(crate) old: usize,
    pub(crate) new: usize,
}

/// What rendering does at the output bound.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    /// Refuse the comparison.
    Refuse,
    /// Keep the hunks and lines that fit, cut at a line.
    Clip,
}

/// A rendered comparison; `clipped` when lines were left out to fit,
/// `whole` when the lines between the common ends were shown replaced whole
/// instead of compared.
pub(crate) struct Rendered {
    pub(crate) data: DiffResult,
    pub(crate) text: String,
    pub(crate) clipped: bool,
    pub(crate) whole: bool,
}

/// The paths a comparison is labelled with.
pub(crate) struct Labels<'a> {
    pub(crate) old: &'a str,
    pub(crate) new: &'a str,
}

/// Group the changed lines into hunks with `context` lines around them and
/// render them within `maximum` bytes.
pub(crate) fn render(
    lines: &[Line<'_>],
    labels: &Labels<'_>,
    context: usize,
    maximum: usize,
    bound: Bound,
    cancel: &CancellationToken,
) -> Result<Rendered, ToolError> {
    let (ranges, additions, deletions) = hunk_ranges(lines, context);
    let mut rendered = Rendered {
        data: DiffResult {
            old_path: labels.old.into(),
            new_path: labels.new.into(),
            changed: additions != 0 || deletions != 0,
            additions,
            deletions,
            context_lines: context,
            hunks: Vec::new(),
        },
        text: String::new(),
        clipped: false,
        whole: false,
    };
    let header = format!("--- {:?}\n+++ {:?}\n", labels.old, labels.new);
    if !ranges.is_empty() && !rendered.fits(&header, maximum, bound)? {
        return Ok(rendered);
    }
    for range in ranges {
        mutation::check_cancel(cancel)?;
        let selected = lines
            .get(range)
            .ok_or_else(|| ToolError::InvalidInput("invalid diff range".into()))?;
        let budget = maximum.saturating_sub(rendered.text.len());
        let (text, hunk, whole) = hunk(selected, budget);
        if !whole && bound == Bound::Refuse {
            return Err(exceeded());
        }
        rendered.text.push_str(&text);
        rendered.data.hunks.extend(hunk);
        if !whole {
            rendered.clipped = true;
            break;
        }
    }
    Ok(rendered)
}

fn exceeded() -> ToolError {
    ToolError::LimitExceeded("diff exceeds max_output_bytes".into())
}

impl Rendered {
    /// Append `text` if it fits `maximum`; otherwise refuse, or mark the
    /// comparison clipped and return false.
    fn fits(&mut self, text: &str, maximum: usize, bound: Bound) -> Result<bool, ToolError> {
        if text.len() <= maximum.saturating_sub(self.text.len()) {
            self.text.push_str(text);
            return Ok(true);
        }
        if bound == Bound::Refuse {
            return Err(exceeded());
        }
        self.clipped = true;
        Ok(false)
    }
}

/// The line ranges of the hunks, with the total lines added and removed.
fn hunk_ranges(lines: &[Line<'_>], context: usize) -> (Vec<Range<usize>>, usize, usize) {
    let mut ranges: Vec<Range<usize>> = Vec::new();
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
    (ranges, additions, deletions)
}

/// One hunk's text and data within `budget` bytes, and whether it is whole.
/// A hunk that does not fit keeps the lines that do, with a header counting
/// them, or nothing when none fits.
fn hunk(selected: &[Line<'_>], budget: usize) -> (String, Option<DiffHunk>, bool) {
    let rendered: Vec<String> = selected.iter().map(line_text).collect();
    let header = |kept: &[Line<'_>]| {
        let (old_start, old_count, new_start, new_count) = span(kept);
        format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n")
    };
    let whole = header(selected).len() + rendered.iter().map(String::len).sum::<usize>();
    let kept = if whole <= budget {
        selected.len()
    } else {
        // The whole hunk's header is at least as long as a part's.
        let mut left = budget.saturating_sub(header(selected).len());
        rendered
            .iter()
            .take_while(|line| {
                let fits = line.len() <= left;
                left = left.saturating_sub(line.len());
                fits
            })
            .count()
    };
    let Some(lines) = selected.get(..kept).filter(|lines| !lines.is_empty()) else {
        return (String::new(), None, false);
    };
    let mut text = header(lines);
    text.extend(rendered.into_iter().take(kept));
    let (old_start, old_count, new_start, new_count) = span(lines);
    let hunk = DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: lines.iter().map(diff_line).collect(),
    };
    (text, Some(hunk), kept == selected.len())
}

/// A hunk's first old line and old line count, then the same for new.
fn span(lines: &[Line<'_>]) -> (usize, usize, usize, usize) {
    let old_count = lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Added)
        .count();
    let new_count = lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Removed)
        .count();
    let (old, new) = lines.first().map_or((0, 0), |first| (first.old, first.new));
    (
        old + usize::from(old_count != 0),
        old_count,
        new + usize::from(new_count != 0),
        new_count,
    )
}

/// A line as the unified text shows it.
fn line_text(line: &Line<'_>) -> String {
    let marker = match line.kind {
        DiffLineKind::Context => ' ',
        DiffLineKind::Added => '+',
        DiffLineKind::Removed => '-',
    };
    let mut text = format!("{marker}{}", line.value);
    if !line.value.ends_with('\n') {
        text.push_str("\n\\ No newline at end of file\n");
    }
    text
}

fn diff_line(line: &Line<'_>) -> DiffLine {
    DiffLine {
        kind: line.kind,
        old_line: (line.kind != DiffLineKind::Added).then_some(line.old + 1),
        new_line: (line.kind != DiffLineKind::Removed).then_some(line.new + 1),
        text: line.value.into(),
    }
}
