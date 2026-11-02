use crate::{SearchCitation, SearchHit, SearchPayload, SearchResult};
use roomkit_tools_core::ToolError;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

// Keep every cited source, then fill remaining slots with consulted sources.
// Reindex source references without modifying any answer text or span.
pub(crate) fn bound_sources(
    sources: &mut Vec<SearchHit>,
    citations: &mut [SearchCitation],
    limit: usize,
) -> Result<usize, ToolError> {
    let mut keep: BTreeSet<usize> = citations.iter().map(|c| c.source_index).collect();
    if keep.len() > limit {
        return Err(ToolError::LimitExceeded(
            "cited sources exceed limit; increase limit rather than dropping citations".into(),
        ));
    }
    for index in 0..sources.len() {
        if keep.len() == limit {
            break;
        }
        keep.insert(index);
    }
    let omitted = sources.len() - keep.len();
    let remap: BTreeMap<usize, usize> = keep
        .into_iter()
        .enumerate()
        .map(|(new, old)| (old, new))
        .collect();
    for citation in citations {
        citation.source_index = *remap
            .get(&citation.source_index)
            .ok_or_else(|| ToolError::UnsupportedResource("invalid citation source".into()))?;
    }
    let mut index = 0;
    sources.retain(|_| {
        let keep = remap.contains_key(&index);
        index += 1;
        keep
    });
    Ok(omitted)
}

pub(crate) fn render(data: &SearchResult) -> Result<String, ToolError> {
    let SearchPayload::Answer {
        answer,
        sources,
        citations,
    } = &data.payload
    else {
        return serde_json::to_string(data)
            .map_err(|_| ToolError::UnsupportedResource("invalid search output".into()));
    };
    let mut text = format!(
        "provider={}, partial={}, omitted_results={}\n\n",
        data.provider, data.partial, data.omitted_results
    );
    let mut insertions: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for citation in citations {
        insertions
            .entry(citation.end)
            .or_default()
            .insert(citation.source_index);
    }
    let mut start = 0;
    for (end, indexes) in insertions {
        text.push_str(answer.get(start..end).ok_or_else(invalid)?);
        for index in indexes {
            let hit = sources.get(index).ok_or_else(invalid)?;
            write!(text, " [source {}](<{}>)", index + 1, link(&hit.url)?)
                .map_err(|_| invalid())?;
        }
        start = end;
    }
    text.push_str(answer.get(start..).ok_or_else(invalid)?);
    if !sources.is_empty() {
        text.push_str("\n\nSources:\n");
    }
    for (index, hit) in sources.iter().enumerate() {
        writeln!(text, "- [source {}](<{}>)", index + 1, link(&hit.url)?).map_err(|_| invalid())?;
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
fn link(url: &str) -> Result<String, ToolError> {
    Ok(url::Url::parse(url)
        .map_err(|_| invalid())?
        .to_string()
        .replace('<', "%3C")
        .replace('>', "%3E"))
}
fn invalid() -> ToolError {
    ToolError::UnsupportedResource("invalid search answer".into())
}
