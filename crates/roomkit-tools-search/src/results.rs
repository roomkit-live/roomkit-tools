//! What a model reads of direct results, and fitting them to the host's
//! output budget.

use crate::{SearchHit, SearchPayload, SearchResult, answer};
use roomkit_tools_core::{Output, ToolError};
use std::fmt::Write as _;

/// The `text` of a result shows its excerpt up to about this many bytes;
/// `data` keeps the excerpt whole.
const TEXT_EXCERPT_BYTES: usize = 600;

/// `data` and its text within `max_bytes` serialized. Direct results that
/// exceed it lose excerpt bytes first, all down to one common length, then
/// results from the end; what is cut is counted and the result is partial.
///
/// # Errors
/// An answer over the budget, since its text and citations are never
/// clipped, and results over it even with none left.
pub(crate) fn bounded(
    mut data: SearchResult,
    max_bytes: usize,
) -> Result<Output<SearchResult>, ToolError> {
    let output = output(data.clone())?;
    if size(&output)? <= max_bytes {
        return Ok(output);
    }
    let SearchPayload::Results { results } = &mut data.payload else {
        return Err(exceeded());
    };
    let hits = std::mem::take(results);
    data.partial = true;
    let cut = Cut {
        data,
        hits,
        max_bytes,
    };
    if let Some(output) = cut.widest_excerpts()? {
        return Ok(output);
    }
    for kept in (0..cut.hits.len()).rev() {
        if let Some(output) = cut.attempt(0, kept)? {
            return Ok(output);
        }
    }
    Err(exceeded())
}

/// Results that do not fit whole, and the budget they must fit.
struct Cut {
    /// The result to fill, already partial.
    data: SearchResult,
    hits: Vec<SearchHit>,
    max_bytes: usize,
}

impl Cut {
    /// The first `kept` hits with excerpts of at most `cap` bytes, if they fit.
    fn attempt(&self, cap: usize, kept: usize) -> Result<Option<Output<SearchResult>>, ToolError> {
        let mut candidate = self.data.clone();
        candidate.omitted_results += self.hits.len() - kept;
        let mut shown: Vec<SearchHit> = self.hits.iter().take(kept).cloned().collect();
        for hit in &mut shown {
            clip(hit, cap)?;
        }
        candidate.payload = SearchPayload::Results { results: shown };
        let output = output(candidate)?;
        Ok((size(&output)? <= self.max_bytes).then_some(output))
    }

    /// Every hit, with the longest common excerpt length that fits, if any.
    /// The output grows with that length, so it is searched by halves, below
    /// the longest excerpt, which did not fit.
    fn widest_excerpts(&self) -> Result<Option<Output<SearchResult>>, ToolError> {
        let longest = self
            .hits
            .iter()
            .filter_map(|hit| hit.content.as_ref().map(String::len))
            .max()
            .unwrap_or(0);
        let (mut low, mut high, mut best) = (0, longest, None);
        while low < high {
            let cap = low + (high - low) / 2;
            match self.attempt(cap, self.hits.len())? {
                Some(output) => {
                    best = Some(output);
                    low = cap + 1;
                }
                None => high = cap,
            }
        }
        Ok(best)
    }
}

/// Keep at most `cap` bytes of a hit's excerpt, on a character boundary,
/// and count the bytes cut.
///
/// # Errors
/// An omitted-byte count past `usize`.
pub(crate) fn clip(hit: &mut SearchHit, cap: usize) -> Result<(), ToolError> {
    if let Some(content) = &mut hit.content {
        let end = boundary(content, cap);
        hit.omitted_bytes = hit
            .omitted_bytes
            .checked_add(content.len() - end)
            .ok_or_else(|| ToolError::LimitExceeded("excerpt byte count overflow".into()))?;
        content.truncate(end);
    }
    Ok(())
}

fn output(data: SearchResult) -> Result<Output<SearchResult>, ToolError> {
    let text = match &data.payload {
        SearchPayload::Results { results } => render(&data, results)?,
        SearchPayload::Answer { .. } | SearchPayload::NotPerformed { .. } => answer::render(&data)?,
    };
    Ok(Output::new(data, text))
}

fn size(output: &Output<SearchResult>) -> Result<usize, ToolError> {
    serde_json::to_vec(output)
        .map(|bytes| bytes.len())
        .map_err(|_| ToolError::UnsupportedResource("invalid search output".into()))
}

fn exceeded() -> ToolError {
    ToolError::LimitExceeded("search output exceeds host byte limit".into())
}

/// One line per result, `N. Title — URL`, then its excerpt on one indented
/// line. Whitespace is folded so page text cannot start a line of its own.
fn render(data: &SearchResult, results: &[SearchHit]) -> Result<String, ToolError> {
    let mut text = format!(
        "provider={}, partial={}, omitted_results={}\n\n",
        data.provider, data.partial, data.omitted_results
    );
    if results.is_empty() {
        text.push_str("No results.\n");
    }
    for (index, hit) in results.iter().enumerate() {
        let url = url::Url::parse(&hit.url)
            .map_err(|_| ToolError::UnsupportedResource("invalid source URL".into()))?;
        let number = index + 1;
        match hit.title.as_deref().map(one_line).filter(|t| !t.is_empty()) {
            Some(title) => writeln!(text, "{number}. {title} — {url}"),
            None => writeln!(text, "{number}. {url}"),
        }
        .map_err(|_| invalid())?;
        let excerpt = hit.content.as_deref().map(one_line).unwrap_or_default();
        if excerpt.is_empty() {
            continue;
        }
        let shown = excerpt
            .get(..boundary(&excerpt, TEXT_EXCERPT_BYTES))
            .unwrap_or_default();
        let more = shown.len() < excerpt.len() || hit.omitted_bytes > 0;
        writeln!(text, "   {shown}{}", if more { " …" } else { "" }).map_err(|_| invalid())?;
    }
    for notice in &data.notices {
        writeln!(text, "Notice: {notice}").map_err(|_| invalid())?;
    }
    if let Some(usage) = &data.usage {
        writeln!(
            text,
            "Observed usage: {}",
            serde_json::to_string(usage).map_err(|_| invalid())?
        )
        .map_err(|_| invalid())?;
    }
    Ok(text)
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The largest character boundary of `value` at or below `cap`.
fn boundary(value: &str, cap: usize) -> usize {
    let mut end = value.len().min(cap);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn invalid() -> ToolError {
    ToolError::UnsupportedResource("invalid search output".into())
}
