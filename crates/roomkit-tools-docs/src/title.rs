//! A document's title: its first `# ` heading outside fenced code, where a
//! `# ` line is a shell or Python comment, not a heading.

/// The text of the first `# ` line of `text` outside fenced code, or `None`
/// when there is none or it is blank.
pub(crate) fn heading(text: &str) -> Option<&str> {
    let mut fence = None;
    text.lines()
        .find_map(|line| {
            let heading = line.strip_prefix("# ").filter(|_| fence.is_none());
            fence = next_fence(fence, line);
            heading
        })
        .map(str::trim)
        .filter(|title| !title.is_empty())
}

/// The fence open after `line`, given the one `open` before it: a line
/// starting with three backticks or tildes opens a fence, and one of the
/// same character closes it.
fn next_fence(open: Option<char>, line: &str) -> Option<char> {
    let marker = line.trim_start();
    let fence = ['`', '~']
        .into_iter()
        .find(|c| marker.starts_with(&c.to_string().repeat(3)));
    match (open, fence) {
        (None, Some(found)) => Some(found),
        (Some(open), Some(found)) if open == found => None,
        (open, _) => open,
    }
}
