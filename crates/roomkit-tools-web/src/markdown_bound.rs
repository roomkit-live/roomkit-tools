//! Upper bound of the Markdown that htmd writes for a DOM, computed from the
//! DOM before any Markdown is written.
//!
//! htmd pads every pipe-table row to the widest cell of each column and to the
//! widest row, and prefixes every line of a blockquote or list item once per
//! enclosing level, so its output can be many times larger than the page. The
//! rules follow htmd 0.5.6 with the converter's options and only ever
//! overestimate: text counts its escapes, elements their markup and the
//! attributes they copy, and quotes and list items a prefix for every line
//! inside them. Text inside `pre`, `code` or a math span counts verbatim, with
//! its whitespace and line breaks, as htmd writes it. A table counts as plain
//! content, or as a padded pipe table where htmd may write one.

mod kind;
mod size;
mod table;

pub(crate) use size::Size;

use html5ever::Attribute;
use kind::{Kind, is_math};
use markup5ever_rcdom::{Handle, NodeData};
use size::count;
use table::{Gathered, Rows};

/// Width of an unordered list marker, `*` and three spaces.
const BULLET_WIDTH: u64 = 4;

/// What an element tells the elements inside it.
#[derive(Clone, Copy, Debug)]
struct Context {
    /// Inside `pre`, `code` or a math span, where htmd keeps text verbatim.
    verbatim: bool,
    /// Class of the nearest `pre`, where a code block may take its language.
    pre_class: Size,
    /// Marker width of list items directly inside.
    item_width: u64,
    /// Inside a `td` or `th`, where htmd writes every table as plain content.
    in_cell: bool,
}

/// An open element whose children are being measured.
#[derive(Debug)]
pub(crate) struct Element {
    kind: Kind,
    /// A `th` or `thead`, which makes htmd consider a pipe table.
    header: bool,
    /// An `ol`, whose direct text children htmd never keeps verbatim.
    ordered: bool,
    context: Context,
    children: Size,
    /// Attribute values the element copies into Markdown.
    attributes: Size,
    table: Gathered,
}

impl Element {
    pub(crate) fn document() -> Self {
        Self::new(
            Kind::Document,
            Context {
                verbatim: false,
                pre_class: Size::default(),
                item_width: BULLET_WIDTH,
                in_cell: false,
            },
            Size::default(),
        )
    }

    fn new(kind: Kind, context: Context, attributes: Size) -> Self {
        Self {
            kind,
            header: false,
            ordered: false,
            context,
            children: Size::default(),
            attributes,
            table: Gathered::default(),
        }
    }

    /// Open a child element of `self`. `attrs` and `children` are what htmd
    /// will see.
    pub(crate) fn open(&self, name: &str, attrs: &[Attribute], children: &[Handle]) -> Self {
        let kind = Kind::of(name, self.context.item_width);
        let class = copied(attrs, &["class"]);
        let context = Context {
            verbatim: self.context.verbatim
                || matches!(name, "pre" | "code")
                || is_math(name, attrs, children),
            pre_class: if name == "pre" {
                class
            } else {
                self.context.pre_class
            },
            item_width: if name == "ol" {
                ordered_width(attrs, children)
            } else {
                BULLET_WIDTH
            },
            in_cell: self.context.in_cell || matches!(name, "td" | "th"),
        };
        let attributes = copied_by(kind, attrs, class, self.context.pre_class);
        let mut element = Self::new(kind, context, attributes);
        element.header = matches!(name, "th" | "thead");
        element.ordered = name == "ol";
        element
    }

    pub(crate) fn add_text(&mut self, text: &str) {
        // htmd renders text directly inside an `ol` as ordinary text, even
        // within `pre`.
        let verbatim = self.context.verbatim && !self.ordered;
        self.children.add(Size::text(text, verbatim));
    }

    /// Finish this element, add its bound to `parent` and return it.
    pub(crate) fn close(mut self, parent: Option<&mut Self>) -> Size {
        let rows = self.table.take_rows();
        let size = self.bound(&rows);
        if let Some(parent) = parent {
            parent.children.add(size);
            let headers = self.header || self.table.headers();
            parent.table.adopt(self.kind, size, rows, headers);
        }
        size
    }

    fn bound(&self, rows: &Rows) -> Size {
        let inner = self.children;
        let mut size = inner;
        size.add(self.attributes);
        let (bytes, lines) = match self.kind {
            Kind::Document => (0, 0),
            Kind::Anchor => (9, 0),
            Kind::Image => (10, 0),
            Kind::Code => {
                let fences = inner.ticks.saturating_add(3).saturating_mul(2);
                size.ticks = size.ticks.saturating_add(fences);
                (fences.saturating_add(4), 2)
            }
            Kind::Item(width) => (prefixed(width, inner.lines).saturating_add(width + 4), 4),
            Kind::Quote => (prefixed(2, inner.lines).saturating_add(4), 4),
            Kind::Heading => (11, 4),
            Kind::Rule => (9, 4),
            Kind::Break => (3, 1),
            Kind::Block | Kind::RowGroup | Kind::Cell | Kind::Caption => (4, 4),
            Kind::Inline => (4, 0),
            Kind::Table => return self.table.bound(size, rows, self.context.in_cell),
        };
        size.bytes = size.bytes.saturating_add(bytes);
        size.lines = size.lines.saturating_add(lines);
        size
    }
}

/// Prefix bytes for every line of content with `lines` line breaks.
fn prefixed(width: u64, lines: u64) -> u64 {
    width.saturating_mul(lines.saturating_add(1))
}

/// Attribute values an element of `kind` copies into Markdown. A code block
/// takes its language from its own class or from the class of its `pre`.
fn copied_by(kind: Kind, attrs: &[Attribute], class: Size, pre_class: Size) -> Size {
    match kind {
        Kind::Anchor => copied(attrs, &["href", "title"]),
        Kind::Image => copied(attrs, &["src", "href", "alt", "title"]),
        Kind::Code => {
            let mut language = class;
            language.add(pre_class);
            language
        }
        Kind::Document
        | Kind::Item(_)
        | Kind::Quote
        | Kind::Table
        | Kind::RowGroup
        | Kind::Cell
        | Kind::Caption
        | Kind::Heading
        | Kind::Rule
        | Kind::Break
        | Kind::Block
        | Kind::Inline => Size::default(),
    }
}

/// Values of the named attributes, as copied into Markdown.
fn copied(attrs: &[Attribute], names: &[&str]) -> Size {
    let mut size = Size::default();
    for attr in attrs {
        if names.contains(&attr.name.local.as_ref()) {
            size.add(Size::attribute(&attr.value));
        }
    }
    size
}

/// Marker width of an ordered list's items: the widest number, a dot, two
/// spaces, and one more column in case htmd's float digit count rounds up.
fn ordered_width(attrs: &[Attribute], children: &[Handle]) -> u64 {
    let start = attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == "start")
        .and_then(|attr| attr.value.parse::<i32>().ok())
        .map_or(1, |start| u64::try_from(start).unwrap_or(1).max(1));
    let items = children
        .iter()
        .filter(|child| {
            matches!(&child.data, NodeData::Element { name, .. } if name.local.as_ref() == "li")
        })
        .count();
    let highest = start.saturating_add(count(items));
    let digits = highest.checked_ilog10().map_or(1, |log| u64::from(log) + 1);
    digits + 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use tokio_util::sync::CancellationToken;

    /// The bound and htmd's Markdown length for `html`.
    fn bound_and_actual(html: &str) -> (u64, u64) {
        let tree = parse::parse(html, &CancellationToken::new()).unwrap();
        let bound = measure(&tree, Element::document(), None).bytes;
        let markdown = htmd::HtmlToMarkdown::new().tree_to_markdown(&tree);
        (bound, count(markdown.len()))
    }

    fn measure(node: &Handle, mut element: Element, parent: Option<&mut Element>) -> Size {
        for child in node.children.borrow().iter() {
            match &child.data {
                NodeData::Element { name, attrs, .. } => {
                    let inner = element.open(
                        name.local.as_ref(),
                        &attrs.borrow(),
                        &child.children.borrow(),
                    );
                    measure(child, inner, Some(&mut element));
                }
                NodeData::Text { contents } => element.add_text(&contents.borrow()),
                NodeData::Document
                | NodeData::Doctype { .. }
                | NodeData::Comment { .. }
                | NodeData::ProcessingInstruction { .. } => {}
            }
        }
        element.close(parent)
    }

    /// Assert that the bound covers htmd's output and stays within `factor`
    /// times it; return the output length.
    fn assert_close(html: &str, factor: u64) -> u64 {
        let (bound, actual) = bound_and_actual(html);
        assert!(
            actual <= bound && bound <= factor * actual,
            "bound {bound} for {actual} bytes: {html}"
        );
        actual
    }

    #[test]
    fn math_spans_count_their_text_verbatim() {
        let spaced = format!("a{}b", " ".repeat(100));
        let math = format!("<p><span class=\"math math-inline\">{spaced}</span></p>");
        assert!(assert_close(&math, 2) > 100);
        // Any other span has its whitespace collapsed, and so has its bound.
        let other = format!("<p><span class=\"math\">{spaced}</span></p>");
        let (bound, actual) = bound_and_actual(&other);
        assert!(
            actual <= bound && bound < 30,
            "bound {bound} for {actual} bytes"
        );
    }

    #[test]
    fn math_lines_take_the_prefix_of_each_enclosing_quote() {
        let html = format!(
            "{}<span class=\"math math-display\">{}</span>",
            "<blockquote>".repeat(10),
            "x\n".repeat(50)
        );
        assert!(assert_close(&html, 2) > 50 * 20);
    }

    #[test]
    fn only_header_tables_outside_cells_are_padded() {
        let rows = "<tr><td>x</td></tr>".repeat(50);
        let wide = "w".repeat(500);
        // A header row pads every row to the widest cell.
        let padded = format!("<table><tr><th>{wide}</th></tr>{rows}</table>");
        assert!(assert_close(&padded, 2) > 50 * 500);
        // Without a header cell, or inside a cell, htmd writes plain content.
        for html in [
            format!("<table><tr><td>{wide}</td></tr>{rows}</table>"),
            format!("<table><tr><td>{padded}</td></tr></table>"),
            format!("<table><tr><th>h</th></tr><tr><td>{padded}</td></tr></table>"),
        ] {
            assert!(assert_close(&html, 2) < 2_000, "{html}");
        }
    }

    #[test]
    fn headers_of_a_nested_table_do_not_pad_the_outer_one() {
        let inner = "<table><tr><th>h</th></tr></table>";
        let html = format!(
            "<table><tr><td>{}</td></tr><tr><td>{inner}</td></tr>{}</table>",
            "w".repeat(500),
            "<tr><td>x</td></tr>".repeat(50)
        );
        assert!(assert_close(&html, 2) < 2_000);
    }
}
