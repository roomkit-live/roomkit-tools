//! Grep's bounded file reads: what one file gives the search, within the
//! per-file bound and what the call may still read.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use roomkit_tools_core::ToolError;

/// Bytes read first to tell binary content from text, so that the rest of a
/// binary file is neither read nor charged.
const HEAD_BYTES: usize = 8 * 1024;

/// What reading one file gave the search.
pub(crate) enum Content {
    /// The file's bytes, to be checked as UTF-8 text.
    Read(Vec<u8>),
    /// A binary file, or one over `max_file_bytes`, skipped after reading
    /// `read` bytes of it.
    Skipped { read: usize },
    /// The file needs more bytes than the call may still read.
    OverBudget,
}

/// Read `path` within `max_file_bytes` and the `remaining` bytes the call
/// may still read; blocking. A file over `max_file_bytes` is skipped unread,
/// and binary content is told from the first [`HEAD_BYTES`].
pub(crate) fn read(
    path: &Path,
    max_file_bytes: usize,
    remaining: usize,
) -> Result<Content, ToolError> {
    let info = std::fs::metadata(path).map_err(|source| ToolError::Io {
        operation: "inspect the search file",
        source,
    })?;
    if !info.is_file() {
        return Err(ToolError::UnsupportedResource(
            "grep accepts regular files only".into(),
        ));
    }
    let size = usize::try_from(info.len()).unwrap_or(usize::MAX);
    if size > max_file_bytes {
        return Ok(Content::Skipped { read: 0 });
    }
    if size.min(HEAD_BYTES) > remaining {
        return Ok(Content::OverBudget);
    }
    let mut file = File::open(path).map_err(|source| ToolError::Io {
        operation: "open the search file",
        source,
    })?;
    let mut bytes = Vec::new();
    read_up_to(&mut file, HEAD_BYTES, &mut bytes)?;
    if binary(&bytes, bytes.len() < HEAD_BYTES) {
        return Ok(Content::Skipped { read: bytes.len() });
    }
    if size > remaining {
        return Ok(Content::OverBudget);
    }
    // The file may have grown since it was inspected: one byte past what may
    // be read tells it.
    let maximum = max_file_bytes.min(remaining);
    read_up_to(
        &mut file,
        maximum.saturating_sub(bytes.len()) + 1,
        &mut bytes,
    )?;
    Ok(if bytes.len() > max_file_bytes {
        Content::Skipped { read: bytes.len() }
    } else if bytes.len() > remaining {
        Content::OverBudget
    } else {
        Content::Read(bytes)
    })
}

/// Append at most `limit` more bytes of `file` to `bytes`.
fn read_up_to(file: &mut File, limit: usize, bytes: &mut Vec<u8>) -> Result<(), ToolError> {
    let limit = u64::try_from(limit).unwrap_or(u64::MAX);
    file.take(limit)
        .read_to_end(bytes)
        .map(|_read| ())
        .map_err(|source| ToolError::Io {
            operation: "read the search file",
            source,
        })
}

/// Whether a file's first bytes are binary: a NUL, or bytes that are not
/// UTF-8. A sequence cut by the end of `head` is text unless `whole`, the
/// head being the entire file.
fn binary(head: &[u8], whole: bool) -> bool {
    if head.contains(&0) {
        return true;
    }
    match std::str::from_utf8(head) {
        Ok(_) => false,
        Err(error) => whole || error.error_len().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_cut_by_the_head_is_text_but_one_cut_by_the_end_is_not() {
        let cut = &"π".as_bytes()[..1];
        assert!(!binary(cut, false));
        assert!(binary(cut, true));
        assert!(binary(b"\xff", false));
        assert!(binary(b"a\0b", false));
        assert!(!binary("text π".as_bytes(), true));
    }
}
