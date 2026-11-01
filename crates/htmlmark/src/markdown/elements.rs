//! Headings, quotes, lists, code, emphasis, links, images, breaks and rules.

use html5ever::Attribute;

use super::{List, Markdown, Pre, Undo};
use crate::Error;
use crate::inline::Inline;
use crate::syntax::{destination, longest_run, title};
use crate::tree::attribute;

impl Markdown<'_> {
    pub(super) fn open_heading(&mut self, name: &str) -> Result<Undo, Error> {
        let level = name.strip_prefix('h').and_then(|level| level.parse().ok());
        if self.flat > 0 || level.is_none() {
            self.flat += 1;
            self.inline().space();
            return Ok(Undo::Heading(None));
        }
        self.boundary()?;
        self.flat += 1;
        self.paragraph.set_paragraph(false);
        Ok(Undo::Heading(level))
    }

    pub(super) fn close_heading(&mut self, level: Option<usize>) -> Result<(), Error> {
        self.flat = self.flat.saturating_sub(1);
        let Some(level) = level else {
            self.inline().space();
            return Ok(());
        };
        self.paragraph.set_paragraph(true);
        let mut line = self.paragraph.take_line();
        Inline::escape_closing_hashes(&mut line);
        if !line.is_empty() {
            self.blocks.line(&format!("{} {line}", "#".repeat(level)))?;
        }
        self.blocks.blank();
        Ok(())
    }

    pub(super) fn open_quote(&mut self) -> Result<Undo, Error> {
        self.boundary()?;
        if self.flat > 0 {
            return Ok(Undo::Quote(false));
        }
        self.blocks.push_quote();
        Ok(Undo::Quote(true))
    }

    /// Close a quote or list item; a blank line follows a quote only.
    pub(super) fn close_container(&mut self, opened: bool, blank: bool) -> Result<(), Error> {
        if !opened {
            self.inline().space();
            return Ok(());
        }
        self.flush()?;
        if blank {
            self.blocks.pop();
            self.blocks.blank();
        } else {
            self.blocks.pop_item();
        }
        Ok(())
    }

    pub(super) fn open_list(&mut self, ordered: bool, attrs: &[Attribute]) -> Result<Undo, Error> {
        let start = attribute(attrs, "start")
            .and_then(|start| start.trim().parse::<u64>().ok())
            .unwrap_or(1);
        // A nested bullet list follows its item's text on the next line; an
        // ordered one needs a blank line to interrupt that text unless its
        // first written item is 1, which an empty first item would change.
        if self.flat > 0 || !self.blocks.in_item() || ordered {
            self.boundary()?;
        } else {
            self.flush()?;
        }
        self.lists.push(List {
            ordered,
            next: start,
        });
        Ok(Undo::List)
    }

    pub(super) fn open_item(&mut self) -> Result<Undo, Error> {
        if self.flat > 0 {
            self.inline().space();
            return Ok(Undo::Item(false));
        }
        self.flush()?;
        let marker = match self.lists.last_mut() {
            Some(List {
                ordered: true,
                next,
            }) => {
                let marker = format!("{}. ", (*next).min(999_999_999));
                *next = next.saturating_add(1);
                marker
            }
            Some(List { ordered: false, .. }) | None => "- ".to_owned(),
        };
        self.blocks.push_item(marker);
        Ok(Undo::Item(true))
    }

    pub(super) fn open_pre(&mut self, attrs: &[Attribute]) -> Result<Undo, Error> {
        if self.flat > 0 {
            self.code = Some(String::new());
            return Ok(Undo::Code);
        }
        self.boundary()?;
        self.pre = Some(Pre {
            text: String::new(),
            language: language(attrs),
        });
        Ok(Undo::Pre)
    }

    /// Write the collected `pre` as a fenced code block, one backtick longer
    /// than the longest run inside.
    pub(super) fn close_pre(&mut self) -> Result<(), Error> {
        let Some(pre) = self.pre.take() else {
            return Ok(());
        };
        let text = pre.text.trim_end_matches('\n');
        if !text.trim().is_empty() {
            let fence = "`".repeat(longest_run(text, '`').max(2) + 1);
            let language = pre.language.unwrap_or_default();
            self.blocks.line(&format!("{fence}{language}"))?;
            for line in text.split('\n') {
                self.blocks.line(line)?;
            }
            self.blocks.line(&fence)?;
        }
        self.blocks.blank();
        Ok(())
    }

    pub(super) fn close_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let edge = |c: Option<char>| c.is_some_and(char::is_whitespace);
        let inline = self.inline();
        if edge(code.chars().next()) {
            inline.space();
        }
        inline.code(&code);
        if edge(code.chars().next_back()) {
            inline.space();
        }
    }

    pub(super) fn marker(&mut self, marker: &str) -> Undo {
        self.inline().open_emphasis(marker);
        Undo::Marker
    }

    pub(super) fn open_link(&mut self, attrs: &[Attribute]) -> Undo {
        let Some(url) = attribute(attrs, "href").and_then(|href| self.options.url(href)) else {
            return Undo::Nothing;
        };
        let close = format!(
            "]({}{})",
            destination(&url),
            title(attribute(attrs, "title"))
        );
        self.inline().open_link(close);
        Undo::Marker
    }

    pub(super) fn image(&mut self, attrs: &[Attribute]) {
        let Some(url) = attribute(attrs, "src").and_then(|src| self.options.url(src)) else {
            return;
        };
        let mut alt = Inline::fragment();
        alt.text(attribute(attrs, "alt").unwrap_or_default());
        let markdown = format!(
            "![{}]({}{})",
            alt.take_line(),
            destination(&url),
            title(attribute(attrs, "title"))
        );
        self.inline().atom(&markdown);
    }

    /// End the paragraph line with a hard break; a space when flat.
    pub(super) fn hard_break(&mut self) -> Result<(), Error> {
        if self.flat > 0 {
            self.inline().space();
            return Ok(());
        }
        if self.paragraph.is_empty() {
            return Ok(());
        }
        let line = self.paragraph.take_line();
        self.blocks.line(&format!("{line}  "))
    }

    pub(super) fn rule(&mut self) -> Result<(), Error> {
        self.boundary()?;
        if self.flat == 0 {
            self.blocks.line("* * *")?;
            self.blocks.blank();
        }
        Ok(())
    }
}

/// The language of a code block from a `language-*` or `lang-*` class,
/// reduced to characters a fence's info string can hold.
pub(super) fn language(attrs: &[Attribute]) -> Option<String> {
    let class = attribute(attrs, "class")?;
    let name = class.split_ascii_whitespace().find_map(|class| {
        class
            .strip_prefix("language-")
            .or_else(|| class.strip_prefix("lang-"))
    })?;
    let name: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.' | '#'))
        .take(32)
        .collect();
    (!name.is_empty()).then_some(name)
}
