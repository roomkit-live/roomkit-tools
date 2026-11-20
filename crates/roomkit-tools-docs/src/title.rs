//! A document's title: its first `# ` heading outside fenced code, where a
//! `# ` line is a shell or Python comment, not a heading.

/// An open fence: the character of its run and the run's length.
type Fence = (char, usize);

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

/// The fence open after `line`, given the one `open` before it, as Markdown
/// reads fences: a run of three or more backticks or tildes opens one; only
/// a run of the same character, at least as long and with nothing after it,
/// closes it, so a ```` ```sh ```` line inside a four-backtick fence, or a
/// ```` ```text ```` line, leaves it open.
fn next_fence(open: Option<Fence>, line: &str) -> Option<Fence> {
    let Some((found, length, rest)) = fence_run(line) else {
        return open;
    };
    match open {
        None => Some((found, length)),
        Some((character, opened))
            if character == found && length >= opened && rest.trim().is_empty() =>
        {
            None
        }
        open => open,
    }
}

/// The fence run `line` starts with: its character, its length and what
/// follows it. A run indented four spaces or a tab is indented code, no
/// fence.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let marker = line.trim_start_matches(' ');
    if line.len() - marker.len() > 3 {
        return None;
    }
    let character = marker.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let rest = marker.trim_start_matches(character);
    let length = marker.len() - rest.len();
    (length >= 3).then_some((character, length, rest))
}
