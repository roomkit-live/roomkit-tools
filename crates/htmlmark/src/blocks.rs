//! Markdown lines and the containers that prefix them.
//!
//! Every line is written with the prefix of the quotes and list items around
//! it: `> ` for a quote, the marker for a list item's first line and as many
//! spaces for the next ones. A blank line between blocks is requested, not
//! written, and only appears once another line follows, so containers never
//! start or end with one. The output limit is checked on every write.

use crate::Error;

/// Output that refuses to grow past its limit.
pub(crate) struct Output {
    text: String,
    max: usize,
}

impl Output {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            text: String::new(),
            max,
        }
    }

    pub(crate) fn push_str(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > self.max.saturating_sub(self.text.len()) {
            return Err(Error::OutputTooLong);
        }
        self.text.push_str(text);
        Ok(())
    }
}

enum Kind {
    Quote,
    Item { marker: String },
}

struct Container {
    kind: Kind,
    /// Whether a line was written inside it.
    started: bool,
}

pub(crate) struct Blocks {
    out: Output,
    containers: Vec<Container>,
    /// Container depth at which a blank line was requested.
    blank: Option<usize>,
    written: bool,
}

impl Blocks {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            out: Output::new(max),
            containers: Vec::new(),
            blank: None,
            written: false,
        }
    }

    pub(crate) fn finish(self) -> String {
        self.out.text
    }

    /// Separate the next line from the previous one by a blank line, at the
    /// current container depth.
    pub(crate) fn blank(&mut self) {
        let depth = self.containers.len();
        self.blank = Some(self.blank.map_or(depth, |blank| blank.min(depth)));
    }

    pub(crate) fn push_quote(&mut self) {
        self.push(Kind::Quote);
    }

    /// Open a list item whose first line starts with `marker`, such as `- `.
    pub(crate) fn push_item(&mut self, marker: String) {
        self.push(Kind::Item { marker });
    }

    fn push(&mut self, kind: Kind) {
        self.containers.push(Container {
            kind,
            started: false,
        });
    }

    /// Close the innermost container; a blank line requested inside it moves
    /// out of it.
    pub(crate) fn pop(&mut self) {
        self.containers.pop();
        let depth = self.containers.len();
        self.blank = self.blank.map(|blank| blank.min(depth));
    }

    /// Close the innermost container, a list item: a blank line requested
    /// inside it is dropped, so items follow each other directly.
    pub(crate) fn pop_item(&mut self) {
        self.containers.pop();
        let depth = self.containers.len();
        self.blank = self.blank.filter(|&blank| blank <= depth);
    }

    /// Whether the innermost container is a list item.
    pub(crate) fn in_item(&self) -> bool {
        matches!(
            self.containers.last(),
            Some(Container {
                kind: Kind::Item { .. },
                ..
            })
        )
    }

    /// Write one line of content inside the current containers.
    pub(crate) fn line(&mut self, content: &str) -> Result<(), Error> {
        if let Some(depth) = self.blank.take()
            && self.started(depth)
        {
            self.out.push_str("\n")?;
            let prefix = self.prefix(depth);
            self.out.push_str(prefix.trim_end())?;
        }
        if self.written {
            self.out.push_str("\n")?;
        }
        let prefix = self.prefix(self.containers.len());
        if content.is_empty() {
            self.out.push_str(prefix.trim_end())?;
        } else {
            self.out.push_str(&prefix)?;
            self.out.push_str(content)?;
        }
        for container in &mut self.containers {
            container.started = true;
        }
        self.written = true;
        Ok(())
    }

    /// Whether the container at `depth` (the document at 0) already holds a
    /// line, so a blank line there separates rather than leads.
    fn started(&self, depth: usize) -> bool {
        match depth.checked_sub(1) {
            None => self.written,
            Some(index) => self
                .containers
                .get(index)
                .is_some_and(|container| container.started),
        }
    }

    fn prefix(&self, depth: usize) -> String {
        let mut prefix = String::new();
        for container in self.containers.iter().take(depth) {
            match &container.kind {
                Kind::Quote => prefix.push_str("> "),
                Kind::Item { marker } if container.started => {
                    prefix.extend(std::iter::repeat_n(' ', marker.len()));
                }
                Kind::Item { marker } => prefix.push_str(marker),
            }
        }
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_separate_but_never_lead_or_trail_a_container() {
        let mut blocks = Blocks::new(usize::MAX);
        blocks.blank();
        blocks.line("intro").unwrap();
        blocks.blank();
        blocks.push_quote();
        blocks.blank();
        blocks.line("quoted").unwrap();
        blocks.blank();
        blocks.push_item("- ".into());
        blocks.line("item").unwrap();
        blocks.blank();
        blocks.line("more").unwrap();
        blocks.blank();
        blocks.pop();
        blocks.pop();
        blocks.blank();
        blocks.line("after").unwrap();
        blocks.blank();
        assert_eq!(
            blocks.finish(),
            "intro\n\n> quoted\n>\n> - item\n>\n>   more\n\nafter"
        );
    }

    #[test]
    fn the_output_limit_counts_every_byte() {
        let mut blocks = Blocks::new(10);
        blocks.push_quote();
        blocks.line("abc").unwrap();
        blocks.line("ab").unwrap();
        assert_eq!(blocks.line(""), Err(Error::OutputTooLong));
        assert_eq!(blocks.finish(), "> abc\n> ab");
    }
}
