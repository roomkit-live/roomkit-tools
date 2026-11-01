//! Grep's blocking content scan: bounded file reads, line matching and paging.
//!
//! The scan runs on a blocking worker so a costly regex never occupies the
//! async executor. It checks cancellation before each file and each line,
//! and hands the regex at most `max_line_bytes` at a time.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::Path;

use globset::GlobSet;
use regex::Regex;
use roomkit_tools_core::{Error, Output};
use tokio_util::sync::CancellationToken;

use crate::ReadLine;
use crate::grep::{GrepInput, GrepLimits, GrepMatch, GrepPage};
use crate::paths::{PathEntry, PathKind, Root};
use crate::worker::check;

/// One prepared search, moved onto a blocking worker.
pub(crate) struct Search {
    pub(crate) root: Root,
    pub(crate) files: Vec<PathEntry>,
    /// Prefix that `include` patterns are relative to.
    pub(crate) prefix: String,
    pub(crate) include: Option<GlobSet>,
    pub(crate) regex: Regex,
    pub(crate) input: GrepInput,
    pub(crate) limits: GrepLimits,
    pub(crate) page: Matches,
}

/// Scan the files in order until the page is full; blocking.
pub(crate) fn run(search: Search, cancel: &CancellationToken) -> Result<Output<GrepPage>, Error> {
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
    let mut ingested = 0;
    for file in files.iter().filter(|file| file.kind == PathKind::File) {
        check(cancel)?;
        let relative = file.path.strip_prefix(&prefix).unwrap_or(&file.path);
        if include
            .as_ref()
            .is_some_and(|pattern| !pattern.is_match(relative))
        {
            continue;
        }
        let resolved = root.resolve_blocking(&file.path)?;
        let remaining = limits.max_total_bytes - ingested;
        let bytes = read_bounded(&resolved, limits.max_file_bytes.min(remaining))?;
        ingested += bytes.len();
        let Some(source) = text(&bytes) else {
            page.data.skipped_files += 1;
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

/// UTF-8 text without its BOM, or `None` for binary or non-UTF-8 content.
fn text(bytes: &[u8]) -> Option<&str> {
    let source = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(source)
        .ok()
        .filter(|source| !source.contains('\0'))
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    let exceeded = || Error::LimitExceeded("grep exceeds max_file_bytes or max_total_bytes".into());
    let info = std::fs::metadata(path).map_err(|source| Error::Io {
        operation: "inspect the search file",
        source,
    })?;
    if !info.is_file() {
        return Err(Error::Unsupported("grep accepts regular files only".into()));
    }
    let maximum = u64::try_from(maximum)
        .map_err(|_| Error::InvalidInput("ingestion limit is too large".into()))?;
    if info.len() > maximum {
        return Err(exceeded());
    }
    let file = File::open(path).map_err(|source| Error::Io {
        operation: "open the search file",
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::Io {
            operation: "read the search file",
            source,
        })?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > maximum) {
        return Err(exceeded());
    }
    Ok(bytes)
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
    fn scan(&self, page: &mut Matches, cancel: &CancellationToken) -> Result<bool, Error> {
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

/// A page of matches being filled within the output budget.
pub(crate) struct Matches {
    pub(crate) data: GrepPage,
    text: String,
    seen: usize,
    offset: usize,
    limit: usize,
    bytes: usize,
}

impl Matches {
    pub(crate) fn new(path: String, input: &GrepInput, limits: GrepLimits) -> Self {
        Self {
            data: GrepPage {
                path,
                matches: Vec::new(),
                truncated: false,
                next_offset: None,
                skipped_files: 0,
                skipped_entries: 0,
                skipped_lines: 0,
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

    /// Add one match; returns false when it starts the next page instead.
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
            return Ok(self.end_page());
        }
        let budget = self.bytes - self.text.len();
        let rendered = render_match(path, line, text, before, after, budget);
        let (rendered, found) = match rendered {
            Some(rendered) => rendered,
            None if self.data.matches.is_empty() => clip_match(path, line, text, budget)?,
            None => return Ok(self.end_page()),
        };
        self.text.push_str(&rendered);
        self.data.matches.push(found);
        Ok(true)
    }

    fn end_page(&mut self) -> bool {
        self.data.truncated = true;
        self.data.next_offset = Some(self.seen);
        false
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

/// Render a match with all requested context, or `None` if it exceeds `budget`.
fn render_match<'a>(
    path: &str,
    line: usize,
    text: &str,
    before: &VecDeque<(usize, &str)>,
    after: impl Iterator<Item = (usize, &'a str)>,
    budget: usize,
) -> Option<(String, GrepMatch)> {
    let mut rendered = String::new();
    let mut context_before = Vec::with_capacity(before.len());
    for &(number, source) in before {
        render_line(&mut rendered, path, number, source, false, budget)?;
        context_before.push(ReadLine {
            number,
            text: source.to_owned(),
        });
    }
    render_line(&mut rendered, path, line, text, true, budget)?;
    let mut context_after = Vec::new();
    for (number, source) in after {
        render_line(&mut rendered, path, number, source, false, budget)?;
        context_after.push(ReadLine {
            number,
            text: source.to_owned(),
        });
    }
    let found = GrepMatch {
        path: path.to_owned(),
        line,
        text: text.to_owned(),
        context_before,
        context_after,
        clipped: false,
    };
    Some((rendered, found))
}

/// A match too large for an empty page: drop its context and cut its text at
/// a UTF-8 boundary so that it fits, flagged as clipped.
fn clip_match(
    path: &str,
    line: usize,
    text: &str,
    budget: usize,
) -> Result<(String, GrepMatch), Error> {
    let prefix = format!("{path:?}:{line}: ");
    let available = budget.checked_sub(prefix.len() + 1).ok_or_else(|| {
        Error::LimitExceeded("max_output_bytes cannot hold one match reference".into())
    })?;
    let mut end = text.len().min(available);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let clipped = text.get(..end).unwrap_or_default();
    let found = GrepMatch {
        path: path.to_owned(),
        line,
        text: clipped.to_owned(),
        context_before: Vec::new(),
        context_after: Vec::new(),
        clipped: true,
    };
    Ok((format!("{prefix}{clipped}\n"), found))
}

/// Append one rendered line, or `None` if it would exceed `budget`.
fn render_line(
    output: &mut String,
    path: &str,
    number: usize,
    text: &str,
    matched: bool,
    budget: usize,
) -> Option<()> {
    let prefix = if matched {
        format!("{path:?}:{number}: ")
    } else {
        format!("{path:?}-{number}- ")
    };
    if prefix.len().saturating_add(text.len()).saturating_add(1) > budget - output.len() {
        return None;
    }
    output.push_str(&prefix);
    output.push_str(text);
    output.push('\n');
    Some(())
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
            Err(Error::Cancelled)
        ));
        assert_eq!(page.data.matches, []);
    }
}
