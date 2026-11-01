//! Emphasis and link markers, written only where a CommonMark reader will
//! see them.
//!
//! CommonMark reads `**` as emphasis only where it touches whitespace or
//! punctuation on its outer side when punctuation is on its inner side:
//! `a**"b"**` and `**Note:**Text` stay literal. Such markers are left out,
//! keeping the text and losing only its emphasis. An opener is decided when
//! content follows it; a closer that follows punctuation is settled when the
//! next character is known, and removed with its opener if that character is
//! a letter or digit. Markers are also kept from touching another pair's
//! markers, whose runs would join if either pair were removed.

use super::Inline;
use crate::syntax::{is_punctuation, is_word};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// Not written yet: waiting for content.
    Waiting,
    /// Written at this byte offset of the line.
    Written(usize),
    /// Left out on this line: CommonMark would not read it as emphasis.
    Dropped,
}

pub(super) struct Marker {
    pub(super) open: String,
    pub(super) close: String,
    /// Emphasis follows CommonMark's flanking rules; a link does not.
    pub(super) emphasis: bool,
    pub(super) state: State,
}

/// An emphasis pair whose closer follows punctuation: it stays only if the
/// next character is not a letter or digit.
pub(super) struct Unsettled {
    open: (usize, usize),
    close: (usize, usize),
}

/// An emphasis closer at the end of the line: the same emphasis opening right
/// after it continues it instead (`**a****b**` is not read as two strong
/// spans).
pub(super) struct Closed {
    marker: String,
    open: usize,
    close: usize,
}

/// What surrounds an emphasis opener about to be written.
struct Context {
    /// The character before it.
    previous: Option<char>,
    /// The character after it: the next marker or the content.
    after: char,
    /// Whether it would touch another pair's closer.
    after_closer: bool,
    /// Whether a `*` emphasis is already open on the line.
    star_open: bool,
}

impl Context {
    /// Whether CommonMark reads an opener here (`star` for `*` and `**`), and
    /// removing a pair later cannot make two runs join.
    fn allows(&self, star: bool) -> bool {
        let previous_word = self.previous.is_some_and(is_word);
        let left_flanking = !(previous_word && is_punctuation(self.after));
        // A `*` run that could also close would close the open emphasis
        // instead of opening its own.
        let could_close = self.previous.is_some_and(|previous| {
            !previous.is_whitespace() && (previous_word || is_punctuation(self.after))
        });
        let ambiguous = star && self.star_open && could_close;
        // Between two code spans, removing this marker again would join
        // their fences.
        let between_code = self.previous == Some('`') && self.after == '`';
        left_flanking && !ambiguous && !between_code && !self.after_closer
    }
}

impl Inline {
    /// Open emphasis such as `**`, written once content follows. Inside the
    /// same emphasis it adds nothing: `****` is not read as nested strong.
    pub(crate) fn open_emphasis(&mut self, marker: &str) {
        let nested = self
            .markers
            .iter()
            .any(|open| open.emphasis && open.open == marker);
        if nested {
            self.push_marker(String::new(), String::new(), false);
        } else {
            self.push_marker(marker.to_owned(), marker.to_owned(), true);
        }
    }

    /// Open a link, closed by `close` (`](url)`), written once content follows.
    /// A link inside another one, which Markdown cannot hold, keeps only its
    /// text.
    pub(crate) fn open_link(&mut self, close: String) {
        if self.markers.iter().any(|marker| marker.open == "[") {
            self.push_marker(String::new(), String::new(), false);
        } else {
            self.push_marker("[".to_owned(), close, false);
        }
    }

    fn push_marker(&mut self, open: String, close: String, emphasis: bool) {
        self.markers.push(Marker {
            open,
            close,
            emphasis,
            state: State::Waiting,
        });
    }

    /// Write the pending space and every marker waiting for content, which
    /// starts with `next`.
    pub(super) fn before_content(&mut self, next: char) {
        self.content = true;
        let waiting = self
            .markers
            .iter()
            .any(|marker| marker.state == State::Waiting);
        if !self.space && !waiting && self.unsettled.is_empty() && self.closed.is_empty() {
            return;
        }
        if self.space {
            self.after_whitespace();
            self.line.push(' ');
            self.space = false;
        }
        let written = self.plan_markers(next);
        // Judged on the content, not on a marker between: that marker may be
        // removed again.
        self.settle(next);
        for index in written {
            let Some(marker) = self.markers.get_mut(index) else {
                continue;
            };
            // `!` right before `[` would turn a link into an image.
            if marker.open.starts_with('[') && ends_with_unescaped(&self.line, '!') {
                self.line.pop();
                self.line.push_str("\\!");
            }
            marker.state = State::Written(self.line.len());
            self.line.push_str(&marker.open);
        }
    }

    /// Whitespace follows the closers just written: they are emphasis.
    pub(super) fn after_whitespace(&mut self) {
        self.unsettled.clear();
        self.closed.clear();
    }

    /// Decide which waiting markers to write before content starting with
    /// `next`, and return their indexes. The same emphasis right after its
    /// own closer continues it instead.
    fn plan_markers(&mut self, next: char) -> Vec<usize> {
        let mut closed = std::mem::take(&mut self.closed);
        let mut context = Context {
            previous: self.line.chars().next_back(),
            after: next,
            after_closer: false,
            star_open: self.markers.iter().any(|marker| {
                marker.emphasis
                    && marker.open.starts_with('*')
                    && matches!(marker.state, State::Written(_))
            }),
        };
        let mut written = Vec::new();
        for index in 0..self.markers.len() {
            let Some(marker) = self
                .markers
                .get(index)
                .filter(|m| m.state == State::Waiting)
            else {
                continue;
            };
            let (emphasis, star, last) = (
                marker.emphasis,
                marker.open.starts_with('*'),
                marker.open.chars().next_back(),
            );
            if last.is_none() {
                self.set_state(index, State::Written(self.line.len()));
                continue;
            }
            if self.reopen(index, &mut closed) {
                context.star_open |= star;
                context.previous = self.line.chars().next_back();
                continue;
            }
            context.after = self.char_after(index).unwrap_or(next);
            context.after_closer = !closed.is_empty();
            if emphasis && !context.allows(star) {
                self.set_state(index, State::Dropped);
                continue;
            }
            context.previous = last;
            closed.clear();
            written.push(index);
        }
        written
    }

    fn set_state(&mut self, index: usize, state: State) {
        if let Some(marker) = self.markers.get_mut(index) {
            marker.state = state;
        }
    }

    /// The first character of the next waiting marker after `index`.
    fn char_after(&self, index: usize) -> Option<char> {
        self.markers
            .iter()
            .skip(index + 1)
            .filter(|marker| marker.state == State::Waiting)
            .find_map(|marker| marker.open.chars().next())
    }

    /// Continue the marker at `index` if its own closer ends the line: the
    /// closer is removed and the earlier opener stands for both.
    fn reopen(&mut self, index: usize, closed: &mut Vec<Closed>) -> bool {
        let line_end = self.line.len();
        let Some(marker) = self.markers.get_mut(index) else {
            return false;
        };
        let Some(reopen) = closed.pop_if(|closed| {
            closed.marker == marker.open && closed.close + closed.marker.len() == line_end
        }) else {
            return false;
        };
        marker.state = State::Written(reopen.open);
        self.line.truncate(reopen.close);
        self.unsettled.retain(|pair| pair.close.0 != reopen.close);
        true
    }

    /// Settle the closers that follow punctuation now that the character
    /// after them is known: before a letter or digit, CommonMark would not
    /// read them as emphasis, so their pairs are removed.
    fn settle(&mut self, following: char) {
        let unsettled = std::mem::take(&mut self.unsettled);
        if !is_word(following) || unsettled.is_empty() {
            return;
        }
        // Positions after the removed markers move.
        self.code = None;
        let mut ranges: Vec<(usize, usize)> = unsettled
            .iter()
            .flat_map(|pair| [pair.open, pair.close])
            .collect();
        ranges.sort_unstable_by_key(|&(start, _)| std::cmp::Reverse(start));
        for (start, len) in ranges {
            let end = start.saturating_add(len);
            if end <= self.line.len()
                && self.line.is_char_boundary(start)
                && self.line.is_char_boundary(end)
            {
                self.line.replace_range(start..end, "");
            }
        }
    }

    /// Close the innermost marker; a pending space stays after it.
    pub(crate) fn close(&mut self) {
        let Some(marker) = self.markers.pop() else {
            return;
        };
        let State::Written(open) = marker.state else {
            return;
        };
        // Spaces HTML keeps at the end move after the closer.
        let trailing = self.split_trailing_spaces();
        let after_punctuation = self.line.chars().next_back().is_some_and(is_punctuation);
        let close = self.line.len();
        self.line.push_str(&marker.close);
        self.line.push_str(&trailing);
        if !trailing.is_empty() {
            self.after_whitespace();
            return;
        }
        if !marker.emphasis {
            return;
        }
        if after_punctuation {
            self.unsettled.push(Unsettled {
                open: (open, marker.open.len()),
                close: (close, marker.close.len()),
            });
        }
        if self
            .closed
            .last()
            .is_some_and(|last| last.close + last.marker.len() != close)
        {
            self.closed.clear();
        }
        self.closed.push(Closed {
            marker: marker.open,
            open,
            close,
        });
    }

    /// Close every written marker at the end of the line; they reopen on the
    /// next one. Closers at the end of a line are emphasis.
    pub(super) fn close_all(&mut self) {
        self.after_whitespace();
        let trailing = self.split_trailing_spaces();
        for marker in self.markers.iter_mut().rev() {
            if matches!(marker.state, State::Written(_)) {
                self.line.push_str(&marker.close);
            }
            marker.state = State::Waiting;
        }
        self.line.push_str(&trailing);
    }

    /// Remove and return the spaces HTML keeps (U+00A0 and the like) that end
    /// the line after the last opener, so closers can go before them.
    fn split_trailing_spaces(&mut self) -> String {
        let last_opener = self
            .markers
            .iter()
            .filter_map(|marker| match marker.state {
                State::Written(open) => Some(open + marker.open.len()),
                State::Waiting | State::Dropped => None,
            })
            .max()
            .unwrap_or(0);
        let end = self
            .line
            .trim_end_matches(char::is_whitespace)
            .len()
            .max(last_opener);
        if end < self.line.len() && self.line.is_char_boundary(end) {
            self.line.split_off(end)
        } else {
            String::new()
        }
    }

    /// Whether any marker is written on the line.
    pub(super) fn has_written_marker(&self) -> bool {
        self.markers
            .iter()
            .any(|marker| matches!(marker.state, State::Written(_)))
    }
}

/// Whether `line` ends with `c` not escaped: preceded by an even number of
/// backslashes.
fn ends_with_unescaped(line: &str, c: char) -> bool {
    let Some(rest) = line.strip_suffix(c) else {
        return false;
    };
    let backslashes = rest
        .chars()
        .rev()
        .take_while(|&found| found == '\\')
        .count();
    backslashes % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::Context;

    fn context(previous: Option<char>, after: char) -> Context {
        Context {
            previous,
            after,
            after_closer: false,
            star_open: false,
        }
    }

    #[test]
    fn openers_follow_commonmark_flanking() {
        // `a**"b"` would not open; `a**b` and ` **"b"` would.
        assert!(!context(Some('a'), '"').allows(true));
        assert!(context(Some('a'), 'b').allows(true));
        assert!(context(Some(' '), '"').allows(true));
        assert!(context(None, '"').allows(true));
        // Inside open `*` emphasis, a run that could close is left out.
        let mut nested = context(Some('a'), 'b');
        nested.star_open = true;
        assert!(!nested.allows(true));
        assert!(nested.allows(false));
        let mut touching = context(Some('*'), 'b');
        touching.after_closer = true;
        assert!(!touching.allows(false));
        assert!(!context(Some('`'), '`').allows(true));
    }
}
