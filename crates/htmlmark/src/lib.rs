//! Bounded HTML to Markdown and plain-text conversion for untrusted pages.
//!
//! `htmlmark` parses with [html5ever](https://docs.rs/html5ever) into its own
//! compact tree and renders Markdown or visible text from it, with limits a
//! hostile page cannot get around:
//!
//! - the parse stops once the document holds more than
//!   [`Options::max_nodes`] nodes or nests deeper than [`Options::max_depth`],
//!   counting the elements the HTML parser recreates for unclosed formatting
//!   tags;
//! - the output is refused as soon as it would exceed
//!   [`Options::max_output_bytes`], measured while it is written rather than
//!   estimated;
//! - [`Options::interrupt`] is polled between parser chunks and during
//!   rendering, so a caller can enforce a deadline or cancellation;
//! - rendering walks the tree without recursion, so nesting cannot exhaust
//!   the stack.
//!
//! ```
//! let markdown = htmlmark::convert(
//!     "<h1>Title</h1><p>Some <b>bold</b> text.</p>",
//!     &htmlmark::Options::default(),
//! )
//! .unwrap();
//! assert_eq!(markdown, "# Title\n\nSome **bold** text.");
//! ```

#![forbid(unsafe_code)]

/// The README's examples compile and run.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
struct ReadmeDoctests;

mod blocks;
mod inline;
mod markdown;
mod sink;
mod syntax;
mod table;
mod text;
mod tree;
mod walk;

use std::fmt;

/// What [`convert`] produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// CommonMark with GitHub-flavored pipe tables and strikethrough.
    #[default]
    Markdown,
    /// The visible words, separated by single spaces.
    Text,
}

/// Rewrites a link or image URL; `None` drops it.
pub type RewriteUrl<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Asked regularly whether to stop; `true` ends the conversion.
pub type Interrupt<'a> = &'a dyn Fn() -> bool;

/// Conversion settings: start from [`Options::default`], then use the
/// `with_*` methods or set fields.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// Markdown (the default) or text.
    pub format: Format,
    /// Most nodes the parsed document may hold. Default 100,000.
    pub max_nodes: usize,
    /// Deepest nesting below the document. Default 128.
    pub max_depth: usize,
    /// Largest output, in bytes. Default 5 MiB.
    pub max_output_bytes: usize,
    /// Applied to every `href` and `src` before use. Without it, URLs are kept
    /// as written; a `None` result renders a link as its text and drops an
    /// image.
    pub rewrite_url: Option<RewriteUrl<'a>>,
    /// Polled between parser chunks and every few hundred rendered nodes.
    pub interrupt: Option<Interrupt<'a>>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            format: Format::Markdown,
            max_nodes: 100_000,
            max_depth: 128,
            max_output_bytes: 5 * 1024 * 1024,
            rewrite_url: None,
            interrupt: None,
        }
    }
}

impl fmt::Debug for Options<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Options")
            .field("format", &self.format)
            .field("max_nodes", &self.max_nodes)
            .field("max_depth", &self.max_depth)
            .field("max_output_bytes", &self.max_output_bytes)
            .field("rewrite_url", &self.rewrite_url.is_some())
            .field("interrupt", &self.interrupt.is_some())
            .finish()
    }
}

impl<'a> Options<'a> {
    /// Set [`Options::format`].
    #[must_use]
    pub fn with_format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Set [`Options::max_nodes`].
    #[must_use]
    pub fn with_max_nodes(mut self, max_nodes: usize) -> Self {
        self.max_nodes = max_nodes;
        self
    }

    /// Set [`Options::max_depth`].
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Set [`Options::max_output_bytes`].
    #[must_use]
    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Set [`Options::rewrite_url`].
    #[must_use]
    pub fn with_rewrite_url(mut self, rewrite_url: RewriteUrl<'a>) -> Self {
        self.rewrite_url = Some(rewrite_url);
        self
    }

    /// Set [`Options::interrupt`].
    #[must_use]
    pub fn with_interrupt(mut self, interrupt: Interrupt<'a>) -> Self {
        self.interrupt = Some(interrupt);
        self
    }
}

impl Options<'_> {
    fn check_interrupt(&self) -> Result<(), Error> {
        if self.interrupt.is_some_and(|interrupt| interrupt()) {
            return Err(Error::Interrupted);
        }
        Ok(())
    }

    fn url(&self, value: &str) -> Option<String> {
        match self.rewrite_url {
            Some(rewrite) => rewrite(value),
            None => Some(value.to_owned()),
        }
    }
}

/// Why a conversion stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The document holds more nodes or nests deeper than the options allow.
    TooLarge,
    /// The output would exceed [`Options::max_output_bytes`].
    OutputTooLong,
    /// [`Options::interrupt`] asked to stop.
    Interrupted,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooLarge => "HTML exceeds the node or depth limit",
            Self::OutputTooLong => "converted output exceeds the byte limit",
            Self::Interrupted => "conversion interrupted",
        })
    }
}

impl std::error::Error for Error {}

/// Convert an HTML document to Markdown or text.
///
/// `head`, `script`, `style`, `noscript`, `template`, `iframe`, `object` and
/// `embed` elements and elements marked `hidden` or `aria-hidden="true"` are
/// left out with everything inside them.
///
/// # Errors
/// [`Error::TooLarge`], [`Error::OutputTooLong`] or [`Error::Interrupted`];
/// nothing is returned in part.
pub fn convert(html: &str, options: &Options<'_>) -> Result<String, Error> {
    let tree = sink::parse(html, options, sink::CHUNK_BYTES)?;
    render(&tree, options)
}

fn render(tree: &tree::Tree, options: &Options<'_>) -> Result<String, Error> {
    match options.format {
        Format::Markdown => markdown::render(tree, options),
        Format::Text => text::render(tree, options),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_parsing_renders_like_a_single_pass() {
        // Text that ends one byte before the first chunk boundary after `tag`.
        let filler = |tag: &str| "a".repeat(sink::CHUNK_BYTES - tag.len() - 1);
        let documents = [
            // A CRLF pair and a two-byte character across a chunk boundary.
            format!("<p>{}\r\nnext</p>", filler("<p>")),
            format!("<p>{}é and more</p>", filler("<p>")),
            format!(
                "<pre>{}\r\n\r\ncode</pre><table><tr><th>h</th></tr></table>",
                filler("<pre>")
            ),
            format!(
                "<!DOCTYPE html><title>t</title><script>{}</script><template><b>x</b></template><select><option selected>o</option></select><p>a<b>b<i>c</b>d</i>e<table>x<tr><td>y</table>",
                filler("")
            ),
        ];
        let options = Options::default();
        for html in documents {
            let chunked = sink::parse(&html, &options, sink::CHUNK_BYTES).unwrap();
            let whole = sink::parse(&html, &options, html.len()).unwrap();
            assert_eq!(
                render(&chunked, &options).unwrap(),
                render(&whole, &options).unwrap()
            );
        }
    }
}
