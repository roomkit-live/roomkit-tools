//! Bound of the Markdown written for text and for copied attribute values.

/// Bound of one subtree's Markdown.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Size {
    /// Bytes, including the line prefixes the subtree adds itself.
    pub(crate) bytes: u64,
    /// Line breaks; every enclosing quote or list item prefixes each line.
    pub(super) lines: u64,
    /// Backticks, which lengthen the code fences around them.
    pub(super) ticks: u64,
    /// Pipes, which a table cell escapes as five bytes.
    pub(super) pipes: u64,
    /// Bytes beyond one per character, which table padding does not count.
    pub(super) wide: u64,
}

impl Size {
    pub(super) fn add(&mut self, other: Self) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.lines = self.lines.saturating_add(other.lines);
        self.ticks = self.ticks.saturating_add(other.ticks);
        self.pipes = self.pipes.saturating_add(other.pipes);
        self.wide = self.wide.saturating_add(other.wide);
    }

    /// Bound of a text node. Verbatim text (preformatted or math) is kept with
    /// its whitespace and line breaks, after a backslash before a leading
    /// fence character. Other text has whitespace runs collapsed, a backslash
    /// before each Markdown special, and one more escape for a leading block
    /// marker or list-number dot.
    pub(super) fn text(text: &str, verbatim: bool) -> Self {
        let first = text.bytes().next().unwrap_or_default();
        let leading = if verbatim {
            matches!(first, b'`' | b'~')
        } else {
            matches!(first, b'=' | b'~' | b'>' | b'-' | b'+' | b'#' | b'0'..=b'9')
        };
        let mut size = Self {
            bytes: u64::from(leading),
            ..Self::default()
        };
        let mut blank = false;
        for byte in text.bytes() {
            let space = byte.is_ascii_whitespace();
            if verbatim || !(space && blank) {
                size.bytes += 1;
            }
            blank = space;
            if !verbatim && matches!(byte, b'\\' | b'*' | b'_' | b'`' | b'[' | b']' | b'<') {
                size.bytes += 1;
            }
            size.count(byte, verbatim);
        }
        size
    }

    /// Bound of an attribute value copied into Markdown: parentheses in link
    /// destinations and quotes in titles gain a backslash, and a destination
    /// with spaces is wrapped in angle brackets.
    pub(super) fn attribute(value: &str) -> Self {
        let mut size = Self {
            bytes: count(value.len()).saturating_add(2),
            ..Self::default()
        };
        for byte in value.bytes() {
            if matches!(byte, b'(' | b')' | b'"') {
                size.bytes += 1;
            }
            size.count(byte, true);
        }
        size
    }

    fn count(&mut self, byte: u8, keep_lines: bool) {
        match byte {
            b'`' => self.ticks += 1,
            b'|' => self.pipes += 1,
            b'\n' if keep_lines => self.lines += 1,
            _ if byte & 0xC0 == 0x80 => self.wide += 1,
            _ => {}
        }
    }
}

pub(super) fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_collapses_whitespace_and_counts_escapes() {
        // "a b*c_" plus a backslash before `*` and `_`.
        let size = Size::text("a \n\t b*c_", false);
        assert_eq!((size.bytes, size.lines), (8, 0));
        // A leading list number gains one escape.
        assert_eq!(Size::text("1. x", false).bytes, 5);
    }

    #[test]
    fn verbatim_text_keeps_every_byte_and_line() {
        let size = Size::text("`a  |\n\nb é", true);
        assert_eq!(size.bytes, 12);
        assert_eq!(
            (size.lines, size.ticks, size.pipes, size.wide),
            (2, 1, 1, 1)
        );
    }

    #[test]
    fn attributes_count_escapes_brackets_and_lines() {
        let size = Size::attribute("/a (1)\n\"t\"");
        assert_eq!((size.bytes, size.lines), (16, 1));
    }
}
