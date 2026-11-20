//! A definition's header on one line, as the outline's text shows it.

use tree_sitter::Node;

/// The comment kinds of the Rust and Python grammars.
const COMMENTS: [&str; 3] = ["line_comment", "block_comment", "comment"];

/// The header of `text` from `first`, the node it starts with, to the byte
/// `end`, on one line: its whitespace collapsed and its comments left out,
/// since a line comment would swallow what follows it once the line breaks
/// go.
pub(crate) fn one_line(text: &str, first: Node<'_>, end: usize) -> String {
    let mut line = String::new();
    let mut at = first.start_byte();
    for (start, stop) in comments(first, end) {
        words(&mut line, text.get(at..start.max(at)).unwrap_or_default());
        at = at.max(stop.min(end));
    }
    words(&mut line, text.get(at..end).unwrap_or_default());
    line
}

/// The byte ranges of the comments that start before `end`, in source
/// order: in `first`, in the siblings after it, and in what they hold.
fn comments(first: Node<'_>, end: usize) -> Vec<(usize, usize)> {
    let mut pending = Vec::new();
    let mut sibling = Some(first);
    while let Some(node) = sibling.filter(|node| node.start_byte() < end) {
        pending.push(node);
        sibling = node.next_sibling();
    }
    let mut found = Vec::new();
    while let Some(node) = pending.pop() {
        if COMMENTS.contains(&node.kind()) {
            found.push((node.start_byte(), node.end_byte()));
            continue;
        }
        let mut cursor = node.walk();
        pending.extend(
            node.children(&mut cursor)
                .filter(|child| child.start_byte() < end),
        );
    }
    found.sort_unstable();
    found
}

/// Append the words of `text` to `line`, one space between each.
fn words(line: &mut String, text: &str) {
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
}
