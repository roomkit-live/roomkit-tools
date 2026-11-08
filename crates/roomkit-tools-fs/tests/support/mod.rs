//! What the tools' tests share.

/// A page's `text` without its last line of facts (`[next_offset=2, ...]`),
/// which a page's byte budget does not count.
pub(crate) fn content(text: &str) -> &str {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let start = body.rfind('\n').map_or(0, |at| at + 1);
    let last = body.get(start..).unwrap_or_default();
    if last.starts_with('[') && last.ends_with(']') {
        text.get(..start).unwrap_or_default()
    } else {
        text
    }
}
