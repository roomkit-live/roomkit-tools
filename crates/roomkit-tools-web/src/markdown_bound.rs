//! Upper bound of the Markdown that htmd writes for a DOM, computed from the
//! DOM before any Markdown is written.
//!
//! htmd pads every pipe-table row to the widest cell of each column and to the
//! widest row, and prefixes every line of a blockquote or list item once per
//! enclosing level, so its output can be many times larger than the page. The
//! rules below follow htmd 0.5.6 with the converter's options and only ever
//! overestimate: text counts its escapes, elements their markup and the
//! attributes they copy, and quotes and list items a prefix for every line
//! inside them. A table counts as the larger of its padded pipe form and its
//! plain content, since htmd chooses between the two from the DOM.

use html5ever::Attribute;
use markup5ever_rcdom::{Handle, NodeData};

/// Bound of one subtree's Markdown.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Size {
    /// Bytes, including the line prefixes the subtree adds itself.
    pub(crate) bytes: u64,
    /// Line breaks; every enclosing quote or list item prefixes each line.
    lines: u64,
    /// Backticks, which lengthen the code fences around them.
    ticks: u64,
    /// Pipes, which a table cell escapes as five bytes.
    pipes: u64,
    /// Bytes beyond one per character, which table padding does not count.
    wide: u64,
}

impl Size {
    fn add(&mut self, other: Self) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.lines = self.lines.saturating_add(other.lines);
        self.ticks = self.ticks.saturating_add(other.ticks);
        self.pipes = self.pipes.saturating_add(other.pipes);
        self.wide = self.wide.saturating_add(other.wide);
    }

    /// Bound of a text node. Preformatted text is kept verbatim with its line
    /// breaks, after a backslash before a leading fence character. Other text
    /// has whitespace runs collapsed, a backslash before each Markdown special,
    /// and one more escape for a leading block marker or list-number dot.
    fn text(text: &str, pre: bool) -> Self {
        let first = text.bytes().next().unwrap_or_default();
        let leading = if pre {
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
            if pre || !(space && blank) {
                size.bytes += 1;
            }
            blank = space;
            if !pre && matches!(byte, b'\\' | b'*' | b'_' | b'`' | b'[' | b']' | b'<') {
                size.bytes += 1;
            }
            size.count(byte, pre);
        }
        size
    }

    /// Bound of an attribute value copied into Markdown: parentheses in link
    /// destinations and quotes in titles gain a backslash, and a destination
    /// with spaces is wrapped in angle brackets.
    fn attribute(value: &str) -> Self {
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

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Width of an unordered list marker, `*` and three spaces.
const BULLET_WIDTH: u64 = 4;

/// Elements htmd renders as blocks separated by blank lines.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "body",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "head",
    "header",
    "html",
    "iframe",
    "legend",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "pre",
    "script",
    "search",
    "section",
    "style",
    "summary",
    "textarea",
    "title",
    "track",
    "ul",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Document,
    Anchor,
    Image,
    Code,
    /// List item with its marker width.
    Item(u64),
    Quote,
    Table,
    /// `tr`, `thead`, `tbody` or `tfoot`.
    RowGroup,
    Cell,
    Caption,
    Heading,
    Rule,
    Break,
    Block,
    Inline,
}

impl Kind {
    fn of(name: &str, item_width: u64) -> Self {
        match name {
            "a" => Self::Anchor,
            "img" => Self::Image,
            "code" => Self::Code,
            "li" => Self::Item(item_width),
            "blockquote" => Self::Quote,
            "table" => Self::Table,
            "tr" | "thead" | "tbody" | "tfoot" => Self::RowGroup,
            "td" | "th" => Self::Cell,
            "caption" => Self::Caption,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Self::Heading,
            "hr" => Self::Rule,
            "br" => Self::Break,
            _ if BLOCKS.contains(&name) => Self::Block,
            _ => Self::Inline,
        }
    }
}

/// What an element tells the elements inside it.
#[derive(Clone, Copy, Debug)]
struct Context {
    /// Inside `pre` or `code`, where htmd keeps text verbatim.
    pre: bool,
    /// Class of the nearest `pre`, where a code block may take its language.
    pre_class: Size,
    /// Marker width of list items directly inside.
    item_width: u64,
}

/// Table rows gathered below a table, as a count and the widest cell bound of
/// each column.
#[derive(Debug, Default)]
struct Rows {
    count: u64,
    widths: Vec<u64>,
}

impl Rows {
    fn merge(&mut self, other: Self) {
        self.count = self.count.saturating_add(other.count);
        for (column, width) in other.widths.into_iter().enumerate() {
            match self.widths.get_mut(column) {
                Some(widest) => *widest = (*widest).max(width),
                None => self.widths.push(width),
            }
        }
    }
}

/// An open element whose children are being measured.
#[derive(Debug)]
pub(crate) struct Element {
    kind: Kind,
    /// An `ol`, whose direct text children htmd never keeps verbatim.
    ordered: bool,
    context: Context,
    children: Size,
    /// Attribute values the element copies into Markdown.
    attributes: Size,
    rows: Rows,
    /// Bounds of the cells directly inside, forming one row.
    cells: Vec<u64>,
    /// Captions directly inside, each on its own line.
    captions: Size,
}

impl Element {
    pub(crate) fn document() -> Self {
        Self::new(
            Kind::Document,
            Context {
                pre: false,
                pre_class: Size::default(),
                item_width: BULLET_WIDTH,
            },
            Size::default(),
        )
    }

    fn new(kind: Kind, context: Context, attributes: Size) -> Self {
        Self {
            kind,
            ordered: false,
            context,
            children: Size::default(),
            attributes,
            rows: Rows::default(),
            cells: Vec::new(),
            captions: Size::default(),
        }
    }

    /// Open a child element of `self`. `attrs` are the values htmd will see.
    pub(crate) fn open(&self, name: &str, attrs: &[Attribute], children: &[Handle]) -> Self {
        let kind = Kind::of(name, self.context.item_width);
        let class = copied(attrs, &["class"]);
        let context = Context {
            pre: self.context.pre || matches!(name, "pre" | "code"),
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
        };
        let attributes = match kind {
            Kind::Anchor => copied(attrs, &["href", "title"]),
            Kind::Image => copied(attrs, &["src", "href", "alt", "title"]),
            Kind::Code => {
                let mut language = class;
                language.add(self.context.pre_class);
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
        };
        let mut element = Self::new(kind, context, attributes);
        element.ordered = name == "ol";
        element
    }

    pub(crate) fn add_text(&mut self, text: &str) {
        // htmd renders text directly inside an `ol` as ordinary text, even
        // within `pre`.
        let verbatim = self.context.pre && !self.ordered;
        self.children.add(Size::text(text, verbatim));
    }

    /// Finish this element, add its bound to `parent` and return it.
    pub(crate) fn close(mut self, parent: Option<&mut Self>) -> Size {
        let rows = self.take_rows();
        let size = self.bound(&rows);
        if let Some(parent) = parent {
            parent.adopt(self.kind, size, rows);
        }
        size
    }

    /// Rows below, plus the cells directly inside as one more row.
    fn take_rows(&mut self) -> Rows {
        let mut rows = std::mem::take(&mut self.rows);
        if !self.cells.is_empty() {
            rows.merge(Rows {
                count: 1,
                widths: std::mem::take(&mut self.cells),
            });
        }
        rows
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
            Kind::Table => return self.table(size, rows),
        };
        size.bytes = size.bytes.saturating_add(bytes);
        size.lines = size.lines.saturating_add(lines);
        size
    }

    /// The larger of the padded pipe table and the plain content: each row
    /// spans every column at its widest cell, plus a header separator row.
    fn table(&self, content: Size, rows: &Rows) -> Size {
        let line = rows.widths.iter().fold(2_u64, |line, width| {
            line.saturating_add(width.saturating_add(3))
        });
        let lines = rows.count.saturating_add(2);
        let pipe = lines
            .saturating_mul(line)
            .saturating_add(self.captions.bytes)
            .saturating_add(content.wide)
            .saturating_add(6);
        let mut size = content;
        size.bytes = pipe.max(content.bytes.saturating_add(4));
        size.lines = lines
            .saturating_add(self.captions.lines)
            .max(content.lines)
            .saturating_add(4);
        size.pipes = size
            .pipes
            .saturating_add(lines.saturating_mul(count(rows.widths.len()).saturating_add(1)));
        size
    }

    fn adopt(&mut self, kind: Kind, size: Size, rows: Rows) {
        self.children.add(size);
        match kind {
            // The table trims the cell's block separators and escapes each pipe
            // as `&#124;`.
            Kind::Cell => self.cells.push(
                size.bytes
                    .saturating_sub(4)
                    .saturating_add(size.pipes.saturating_mul(4)),
            ),
            Kind::RowGroup => self.rows.merge(rows),
            Kind::Caption => self.captions.add(Size {
                bytes: size.bytes.saturating_add(1),
                lines: size.lines.saturating_add(1),
                ..Size::default()
            }),
            Kind::Document
            | Kind::Anchor
            | Kind::Image
            | Kind::Code
            | Kind::Item(_)
            | Kind::Quote
            | Kind::Table
            | Kind::Heading
            | Kind::Rule
            | Kind::Break
            | Kind::Block
            | Kind::Inline => {}
        }
    }
}

/// Prefix bytes for every line of content with `lines` line breaks.
fn prefixed(width: u64, lines: u64) -> u64 {
    width.saturating_mul(lines.saturating_add(1))
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
