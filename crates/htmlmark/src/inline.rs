//! One line of inline Markdown: escaped text, collapsed whitespace, and
//! emphasis or link markers written only around visible content.
//!
//! An opening marker waits until content follows it, so an empty `<b></b>`
//! writes nothing, and a space at the edge of an element moves outside its
//! markers (`a<b> b </b>c` gives `a **b** c`). When a line ends, open markers
//! are closed and reopened on the next line: emphasis cannot span blocks.

mod markers;

use markers::{Closed, Marker, Unsettled};

use crate::syntax::{code_span, is_html_space, ordinal, starts_block};

pub(crate) struct Inline {
    line: String,
    /// A collapsible space waits for the next content.
    space: bool,
    markers: Vec<Marker>,
    unsettled: Vec<Unsettled>,
    /// The closers ending the line, innermost first.
    closed: Vec<Closed>,
    /// Where the last code span ends. Another one right after it would join
    /// its backtick fences, so it is written as text.
    code: Option<usize>,
    /// Whether text or an atom was written on this line; markers alone do not
    /// count, since a marker may be removed again.
    content: bool,
    /// Whether the line starts a paragraph line, where `# x`, `- x` or `1. x`
    /// would turn into a heading or list; false in headings and table cells.
    paragraph: bool,
}

impl Inline {
    /// A paragraph line.
    pub(crate) fn paragraph() -> Self {
        Self {
            paragraph: true,
            ..Self::fragment()
        }
    }

    /// Inline content that never starts a line of its own: a heading, a table
    /// cell, an image description.
    pub(crate) fn fragment() -> Self {
        Self {
            line: String::new(),
            space: false,
            markers: Vec::new(),
            unsettled: Vec::new(),
            closed: Vec::new(),
            code: None,
            content: false,
            paragraph: false,
        }
    }

    pub(crate) fn set_paragraph(&mut self, paragraph: bool) {
        self.paragraph = paragraph;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    /// Add text, collapsing whitespace and escaping what Markdown would read
    /// as syntax.
    pub(crate) fn text(&mut self, text: &str) {
        let mut chars = text.char_indices().peekable();
        while let Some((index, c)) = chars.next() {
            if is_html_space(c) {
                self.space();
                continue;
            }
            if c.is_whitespace() {
                self.visible_space(c);
                continue;
            }
            let line_start = self.paragraph && !self.content;
            self.before_content(c);
            let next = chars.peek().map(|&(_, next)| next);
            if line_start {
                let rest = text.get(index..).unwrap_or_default();
                if let Some((number, delimiter)) = ordinal(rest) {
                    // `1. x` or `1) x` would start a list: escape the
                    // delimiter, then skip the other digits and it.
                    self.line.push_str(number);
                    self.line.push('\\');
                    self.line.push(delimiter);
                    for _ in 0..number.len() {
                        chars.next();
                    }
                    continue;
                }
                if starts_block(c, next) {
                    self.line.push('\\');
                }
            }
            self.escape(c, next);
        }
    }

    fn escape(&mut self, c: char, next: Option<char>) {
        let previous = self.line.chars().next_back();
        let alphanumeric = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let escaped = match c {
            '\\' | '*' | '`' | '[' | ']' | '<' | '~' => true,
            // Inside a word, `_` cannot start or end emphasis.
            '_' => !(alphanumeric(previous) && alphanumeric(next)),
            // `&amp;` written as text would turn back into `&`.
            '&' => next.is_none_or(|next| next.is_alphanumeric() || next == '#'),
            _ => false,
        };
        if escaped {
            self.line.push('\\');
        }
        self.line.push(c);
    }

    /// Request a collapsible space before the next content.
    pub(crate) fn space(&mut self) {
        self.after_whitespace();
        if !self.line.is_empty() || self.has_written_marker() {
            self.space = true;
        }
    }

    /// Add a space HTML keeps, such as U+00A0, outside any marker still
    /// waiting: emphasis cannot start or end next to whitespace.
    fn visible_space(&mut self, c: char) {
        self.after_whitespace();
        if self.space {
            self.line.push(' ');
            self.space = false;
        }
        self.line.push(c);
        self.content = true;
    }

    /// Add Markdown that is already escaped, such as an image or code span.
    pub(crate) fn atom(&mut self, markdown: &str) {
        let Some(first) = markdown.chars().next() else {
            return;
        };
        self.before_content(first);
        self.line.push_str(markdown);
    }

    /// Add a code span. It is written as escaped text instead in two cases:
    /// right after another code span, whose backtick fences it would join
    /// (joining the two would rewrite the whole span each time); and inside a
    /// link when it holds `]`, since at the start of a line `[` up to a `]:`
    /// reads as a link definition and a code span cannot escape the bracket.
    pub(crate) fn code(&mut self, code: &str) {
        let after_code = self.code == Some(self.line.len()) && !self.space;
        let in_link = self.markers.iter().any(|marker| marker.open == "[");
        if after_code || (in_link && code.contains(']')) {
            self.code = None;
            self.text(code);
            return;
        }
        let span = code_span(code);
        let Some(first) = span.chars().next() else {
            return;
        };
        self.before_content(first);
        // A closer continued by the same emphasis is removed again, which can
        // bring the previous code span back to the end of the line.
        if self.code == Some(self.line.len()) {
            self.code = None;
            self.text(code);
            return;
        }
        self.line.push_str(&span);
        self.code = Some(self.line.len());
    }

    /// Escape a closing sequence of `#` that would end a heading line.
    pub(crate) fn escape_closing_hashes(line: &mut String) {
        let content = line.trim_end_matches('#');
        if content.len() < line.len() && (content.is_empty() || content.ends_with(' ')) {
            line.insert(content.len(), '\\');
        }
    }

    /// End the line: close the open markers, which reopen on the next line,
    /// and return its content.
    pub(crate) fn take_line(&mut self) -> String {
        self.close_all();
        self.code = None;
        self.content = false;
        self.space = false;
        std::mem::take(&mut self.line)
    }
}
