//! Grep's blocking content scan: bounded file reads and line matching, into
//! a page of matches.
//!
//! The scan runs on a blocking worker so a costly regex never occupies the
//! async executor. It checks cancellation before each file and each line,
//! and hands the regex at most `max_line_bytes` at a time.

use std::collections::VecDeque;

use regex::Regex;
use roomkit_tools_core::{Output, ToolError};
use tokio_util::sync::CancellationToken;

use crate::grep::{GrepInput, GrepLimits, GrepPage};
use crate::grep_include::Include;
use crate::grep_page::Matches;
use crate::grep_read::{self, Content};
use crate::paths::{PathEntry, PathKind, Root, Stop};
use crate::worker::check;

/// One prepared search, moved onto a blocking worker.
pub(crate) struct Search {
    pub(crate) root: Root,
    pub(crate) files: Vec<PathEntry>,
    /// Prefix that `include` paths are relative to.
    pub(crate) prefix: String,
    pub(crate) include: Option<Include>,
    pub(crate) regex: Regex,
    pub(crate) input: GrepInput,
    pub(crate) limits: GrepLimits,
    pub(crate) page: Matches,
}

/// Scan the files in order until the page is full; blocking.
pub(crate) fn run(
    search: Search,
    cancel: &CancellationToken,
) -> Result<Output<GrepPage>, ToolError> {
    let Search {
        root,
        files,
        prefix,
        include,
        regex,
        input,
        limits,
        mut page,
    } = search;
    let mut reader = Reader {
        root: &root,
        limits,
        ingested: 0,
    };
    for file in files.iter().filter(|file| file.kind == PathKind::File) {
        check(cancel)?;
        if include
            .as_ref()
            .is_some_and(|include| !include.matches(file, &prefix))
        {
            continue;
        }
        let bytes = match reader.read(&file.path)? {
            Content::Read(bytes) => bytes,
            Content::Skipped { .. } => {
                page.skip(&file.path);
                continue;
            }
            Content::OverBudget => {
                page.stop(Stop {
                    bound: "max_total_bytes",
                    path: file.path.clone(),
                });
                break;
            }
        };
        let Some(source) = text(&bytes) else {
            page.skip(&file.path);
            continue;
        };
        let lines = Lines {
            path: &file.path,
            source,
            regex: &regex,
            input: &input,
            max_line_bytes: limits.max_line_bytes,
        };
        if !lines.scan(&mut page, cancel)? {
            break;
        }
    }
    page.finish()
}

/// Reads a search's files within its byte bounds.
struct Reader<'a> {
    root: &'a Root,
    limits: GrepLimits,
    /// Bytes read so far, charged to `max_total_bytes`.
    ingested: usize,
}

impl Reader<'_> {
    /// Read the file at `path`, charging what was read; blocking.
    fn read(&mut self, path: &str) -> Result<Content, ToolError> {
        let resolved = self.root.resolve_blocking(path)?;
        let remaining = self.limits.max_total_bytes.saturating_sub(self.ingested);
        let content = grep_read::read(&resolved, self.limits.max_file_bytes, remaining)?;
        self.ingested += match &content {
            Content::Read(bytes) => bytes.len(),
            Content::Skipped { read } => *read,
            Content::OverBudget => 0,
        };
        Ok(content)
    }
}

/// UTF-8 text without its BOM, or `None` for binary or non-UTF-8 content.
fn text(bytes: &[u8]) -> Option<&str> {
    let source = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(source)
        .ok()
        .filter(|source| !source.contains('\0'))
}

/// The lines of one file and what to look for in them.
struct Lines<'a> {
    path: &'a str,
    source: &'a str,
    regex: &'a Regex,
    input: &'a GrepInput,
    max_line_bytes: usize,
}

impl Lines<'_> {
    /// Push this file's matches; returns false once the page is full.
    fn scan(&self, page: &mut Matches, cancel: &CancellationToken) -> Result<bool, ToolError> {
        let context = self.input.context_before;
        let mut preceding = VecDeque::with_capacity(context);
        let mut lines = self.source.lines().enumerate();
        while let Some((index, line)) = lines.next() {
            check(cancel)?;
            if line.len() > self.max_line_bytes {
                page.data.skipped_lines += 1;
            } else if self.regex.is_match(line) {
                let after = lines
                    .clone()
                    .take(self.input.context_after)
                    .map(|(i, text)| (i + 1, text));
                if !page.push(self.path, index + 1, line, &preceding, after)? {
                    return Ok(false);
                }
            }
            if context > 0 {
                if preceding.len() == context {
                    preceding.pop_front();
                }
                preceding.push_back((index + 1, line));
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_worker_stops_before_reading_or_matching() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let input = GrepInput::new("hit");
        let mut page = Matches::new(".".into(), &input, GrepLimits::default());
        let lines = Lines {
            path: "a",
            source: "hit\nhit\n",
            regex: &Regex::new("hit").unwrap(),
            input: &input,
            max_line_bytes: 1024,
        };
        assert!(matches!(
            lines.scan(&mut page, &cancel),
            Err(ToolError::Cancelled)
        ));
        assert_eq!(page.data.matches, []);
    }
}
