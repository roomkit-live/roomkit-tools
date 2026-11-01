//! Line endings of model-supplied text for mutations of CRLF files.

use std::borrow::Cow;

/// Whether `text` has at least one line ending and every one of them is CRLF.
pub(crate) fn uniformly_crlf(text: &str) -> bool {
    let mut endings = text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .peekable();
    endings.peek().is_some() && endings.all(|line| line.ends_with("\r\n"))
}

/// Model-supplied text in a file's line endings. `read` shows CRLF lines
/// without their `\r`, so for a file that uses CRLF throughout (`crlf`), bare
/// LF endings in `text` become CRLF; existing CRLF is kept. Any other file
/// keeps exact matching.
///
/// A line whose own text ends in a bare `\r` (`a\r\r\n` in the file, shown
/// by `read` as `a\r`) needs its exact ending: `a\r` followed by LF reads as
/// an existing CRLF and is kept as `a\r\n`, which does not match. Taking it as
/// `a\r\r\n` instead would misread text that already uses CRLF.
pub(crate) fn in_file_endings(crlf: bool, text: &str) -> Cow<'_, str> {
    if !crlf || !text.contains('\n') {
        return Cow::Borrowed(text);
    }
    let mut converted = String::with_capacity(text.len() + text.len() / 8);
    for line in text.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(body) if !body.ends_with('\r') => {
                converted.push_str(body);
                converted.push_str("\r\n");
            }
            _ => converted.push_str(line),
        }
    }
    Cow::Owned(converted)
}
