//! How htmd writes each element, as far as the bound needs to know.

use html5ever::Attribute;
use markup5ever_rcdom::{Handle, NodeData};

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
pub(super) enum Kind {
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
    /// Any other element, including a math span: emphasis markers or math
    /// delimiters add at most four bytes.
    Inline,
}

impl Kind {
    pub(super) fn of(name: &str, item_width: u64) -> Self {
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

/// Whether htmd writes the element as math: a `span` whose only attribute is
/// the class `math math-inline` or `math math-display` and whose only child is
/// text. htmd writes that text verbatim, whitespace and line breaks included,
/// between `$` or `$$` delimiters.
pub(super) fn is_math(name: &str, attrs: &[Attribute], children: &[Handle]) -> bool {
    let math_class = |attr: &Attribute| {
        attr.name.local.as_ref() == "class"
            && matches!(
                attr.value.as_ref(),
                "math math-inline" | "math math-display"
            )
    };
    name == "span"
        && matches!(attrs, [attr] if math_class(attr))
        && matches!(children, [child] if matches!(child.data, NodeData::Text { .. }))
}
