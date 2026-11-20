//! A page of grep matches: paging by offset and limit, rendering within the
//! output budget, and the facts that close the text.

use std::collections::VecDeque;
use std::num::NonZeroUsize;

use roomkit_tools_core::{Output, ToolError};

use crate::ReadLine;
use crate::grep::{GrepInput, GrepLimits, GrepMatch, GrepPage};
use crate::paths::Stop;

/// A page of matches being filled within the output budget.
pub(crate) struct Matches {
    pub(crate) data: GrepPage,
    /// The first file skipped, named in the facts.
    first_skipped: Option<String>,
    /// Where a host bound stopped the search before the page was full.
    stopped: Option<Stop>,
    /// The context used, stated when the request was capped.
    context: Option<(usize, usize)>,
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
            first_skipped: None,
            stopped: None,
            context: None,
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
    pub(crate) fn push<'a>(
        &mut self,
        path: &str,
        line: usize,
        text: &str,
        before: &VecDeque<(usize, &str)>,
        after: impl Iterator<Item = (usize, &'a str)>,
    ) -> Result<bool, ToolError> {
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

    /// State in the facts the context `input` holds, the request capped to
    /// the host's bound.
    pub(crate) fn state_context(&mut self, input: &GrepInput) {
        self.context = Some((input.context_before, input.context_after));
    }

    /// Count a file skipped as binary, non-UTF-8 or over `max_file_bytes`.
    pub(crate) fn skip(&mut self, path: &str) {
        self.data.skipped_files += 1;
        self.first_skipped
            .get_or_insert_with(|| format!("{path:?}"));
    }

    /// End the search at a host bound: the matches found so far are the
    /// last ones this call can return. Of the walk's stop and the byte
    /// budget's, the one named is the first in path order, where the search
    /// began to miss files.
    pub(crate) fn stop(&mut self, stop: Stop) {
        self.data.truncated = true;
        if self
            .stopped
            .as_ref()
            .is_none_or(|earlier| stop.path < earlier.path)
        {
            self.stopped = Some(stop);
        }
    }

    fn end_page(&mut self) -> bool {
        self.data.truncated = true;
        self.data.next_offset = Some(self.seen);
        false
    }

    pub(crate) fn finish(self) -> Result<Output<GrepPage>, ToolError> {
        // A stopped search cannot tell whether matches lie past the offset.
        if self.stopped.is_none()
            && self.offset > self.seen
            && !(self.offset == 1 && self.seen == 0)
        {
            return Err(ToolError::InvalidInput(
                "offset is beyond the last match".into(),
            ));
        }
        let mut text = self.text;
        if text.is_empty() {
            text.push_str("no matches\n");
        }
        let count = |skipped: usize| (skipped > 0).then(|| skipped.to_string());
        // Only a page's single match is ever clipped.
        let clipped = self
            .data
            .matches
            .iter()
            .any(|found| found.clipped)
            .then(|| "true".to_owned());
        let [truncated_by, stopped_at] = Stop::facts(self.stopped.as_ref());
        roomkit_tools_core::append_facts(
            &mut text,
            [
                (
                    "next_offset",
                    self.data.next_offset.map(|next| next.to_string()),
                ),
                truncated_by,
                stopped_at,
                (
                    "context_before",
                    self.context.map(|(before, _)| before.to_string()),
                ),
                (
                    "context_after",
                    self.context.map(|(_, after)| after.to_string()),
                ),
                ("clipped", clipped),
                ("skipped_files", count(self.data.skipped_files)),
                ("first_skipped_file", self.first_skipped),
                ("skipped_entries", count(self.data.skipped_entries)),
                ("skipped_lines", count(self.data.skipped_lines)),
            ],
        );
        Ok(Output::new(self.data, text))
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
) -> Result<(String, GrepMatch), ToolError> {
    let prefix = format!("{path:?}:{line}: ");
    let available = budget.checked_sub(prefix.len() + 1).ok_or_else(|| {
        ToolError::LimitExceeded("max_output_bytes cannot hold one match reference".into())
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
