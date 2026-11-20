//! What a model reads of direct results, and fitting them to the host's
//! output budget.

use crate::{SearchHit, SearchPayload, SearchResult, answer};
use roomkit_tools_core::{Output, ToolError};
use std::fmt::Write as _;

/// The `text` of a result shows its excerpt up to about this many bytes;
/// `data` keeps the excerpt whole.
const TEXT_EXCERPT_BYTES: usize = 600;

/// `data` and its text within `max_bytes` serialized. Direct results that
/// exceed it first lose, in rank order, each result that does not fit even
/// without its excerpt (a page can give itself a huge title), then excerpt
/// bytes, all down to the longest common length that fits; what is cut is
/// counted and the result is partial.
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
    let mut cut = Cut {
        data,
        hits,
        dropped: 0,
        max_bytes,
    };
    cut.drop_misfits()?;
    cut.widest_excerpts()?.ok_or_else(exceeded)
}

/// Results that do not fit whole, and the budget they must fit.
struct Cut {
    /// The result to fill, without its hits.
    data: SearchResult,
    /// The hits kept so far, in rank order, with their excerpts whole.
    hits: Vec<SearchHit>,
    /// Hits left out because they do not fit.
    dropped: usize,
    max_bytes: usize,
}

impl Cut {
    /// The kept hits with excerpts of at most `cap` bytes, if they fit. Each
    /// excerpt is cut as it is copied, so a long one is never copied whole.
    fn attempt(&self, cap: usize) -> Result<Option<Output<SearchResult>>, ToolError> {
        let mut candidate = self.data.clone();
        let mut cut_any = self.dropped > 0;
        let mut results = Vec::with_capacity(self.hits.len());
        for hit in &self.hits {
            let (shown, cut) = clipped(hit, cap)?;
            cut_any |= cut;
            results.push(shown);
        }
        candidate.omitted_results += self.dropped;
        candidate.partial |= cut_any;
        candidate.payload = SearchPayload::Results { results };
        let output = output(candidate)?;
        Ok((size(&output)? <= self.max_bytes).then_some(output))
    }

    /// Leave out, in rank order, each hit that does not fit beside the ones
    /// kept before it once every excerpt is empty. While a hit is weighed,
    /// those not yet weighed count as omitted, so the count the final output
    /// shows is never longer than the one measured.
    fn drop_misfits(&mut self) -> Result<(), ToolError> {
        if self.attempt(0)?.is_some() {
            return Ok(());
        }
        let hits = std::mem::take(&mut self.hits);
        let mut unweighed = hits.len();
        for hit in hits {
            unweighed -= 1;
            self.hits.push(hit);
            let dropped = self.dropped;
            self.dropped += unweighed;
            let fits = self.attempt(0)?.is_some();
            self.dropped = dropped;
            if !fits {
                self.hits.pop();
                self.dropped += 1;
            }
        }
        Ok(())
    }

    /// The kept hits with the longest common excerpt length that fits, if
    /// any. The output grows with that length, so it is searched by halves.
    fn widest_excerpts(&self) -> Result<Option<Output<SearchResult>>, ToolError> {
        let longest = self
            .hits
            .iter()
            .filter_map(|hit| hit.content.as_ref().map(String::len))
            .max()
            .unwrap_or(0);
        let (mut low, mut high, mut best) = (0, longest + 1, None);
        while low < high {
            let cap = low + (high - low) / 2;
            match self.attempt(cap)? {
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

/// A copy of `hit` with at most `cap` bytes of its excerpt, and whether any
/// were cut.
fn clipped(hit: &SearchHit, cap: usize) -> Result<(SearchHit, bool), ToolError> {
    let (content, cut) = match hit.content.as_deref() {
        Some(content) => {
            let end = boundary(content, cap);
            let kept = content.get(..end).unwrap_or_default();
            (Some(kept.to_owned()), content.len() - end)
        }
        None => (None, 0),
    };
    let omitted_bytes = hit
        .omitted_bytes
        .checked_add(cut)
        .ok_or_else(|| ToolError::LimitExceeded("excerpt byte count overflow".into()))?;
    let shown = SearchHit {
        url: hit.url.clone(),
        title: hit.title.clone(),
        content,
        omitted_bytes,
    };
    Ok((shown, cut > 0))
}

/// Keep at most `cap` bytes of a hit's excerpt, on a character boundary,
/// and count the bytes cut.
///
/// # Errors
/// An omitted-byte count past `usize`.
pub(crate) fn clip(hit: &mut SearchHit, cap: usize) -> Result<(), ToolError> {
    *hit = clipped(hit, cap)?.0;
    Ok(())
}

fn output(data: SearchResult) -> Result<Output<SearchResult>, ToolError> {
    let text = match &data.payload {
        SearchPayload::Results { results } => render(&data, results)?,
        SearchPayload::Answer { .. } | SearchPayload::NotPerformed { .. } => answer::render(&data)?,
    };
    Ok(Output::new(data, text))
}

/// The serialized size of `output`, counted without keeping the bytes.
fn size(output: &Output<SearchResult>) -> Result<usize, ToolError> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, output).map_err(|_| invalid())?;
    Ok(counter.0)
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
    // A limit is at least 1, so results left out with none shown were left
    // out for the budget, not the count.
    if results.is_empty() && data.omitted_results > 0 {
        text.push_str("No results shown: none fits the output budget.\n");
    } else if results.is_empty() {
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
