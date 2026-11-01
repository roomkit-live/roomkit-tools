//! Incremental HTML parsing into the converter's DOM, bounded while it runs.
//!
//! The HTML parser recreates every open formatting element (`b`, `a`, `font`,
//! ...) inside each new block that follows them, so a short page can expand
//! into far more nodes than its source has tags. The sink below counts nodes
//! and nesting as the parser creates and inserts them. Past a limit it stops
//! building the tree, and the input, fed in small chunks, is abandoned at the
//! next chunk boundary. Cancellation is checked between chunks too.
//!
//! Each repeated `<html>` or `<body>` tag merges its attributes into the
//! existing element after comparing them with every attribute already there,
//! so the sink counts a merge as that many nodes. The tokenizer also compares
//! each attribute of a tag with the earlier ones of the same tag, before the
//! sink sees the element: a single tag with tens of thousands of attributes
//! takes seconds to tokenize, and only cancellation or the caller's deadline
//! stops that work, at the next chunk boundary.

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, ExpandedName, ParseOpts, QualName, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use roomkit_tools_core::Error;
use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Weak;
use tokio_util::sync::CancellationToken;

/// Most nodes a document may have.
pub(crate) const MAX_NODES: usize = 100_000;
/// Deepest nesting below the document node.
pub(crate) const MAX_DEPTH: usize = 128;
/// Input fed to the parser between limit and cancellation checks.
const CHUNK_BYTES: usize = 1024;

pub(crate) fn too_large() -> Error {
    Error::LimitExceeded("HTML exceeds 100000 nodes or 128 levels".into())
}

/// Parse with the same options as the converter, refusing a document whose
/// parse creates more than [`MAX_NODES`] nodes or inserts one deeper than
/// [`MAX_DEPTH`]. Depth is checked at insertion; nodes the parser moves later
/// are checked again by the caller's walk.
pub(crate) fn parse(source: &str, cancel: &CancellationToken) -> Result<Handle, Error> {
    let mut parser = parse_document(BoundedDom::default(), ParseOpts::default());
    let mut rest = source;
    while !rest.is_empty() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let (chunk, tail) = rest
            .split_at_checked(rest.floor_char_boundary(CHUNK_BYTES))
            .unwrap_or((rest, ""));
        parser.process(StrTendril::from_slice(chunk));
        if parser.tokenizer.sink.sink.exceeded.get() {
            return Err(too_large());
        }
        rest = tail;
    }
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let BoundedDom { dom, exceeded, .. } = parser.finish();
    if exceeded.get() {
        return Err(too_large());
    }
    Ok(dom.document)
}

/// `RcDom` that stops building once the document exceeds its bounds.
///
/// Past a bound, every tree mutation is ignored and new nodes are detached and
/// carry no attributes or text. The parser's own state stays consistent, since
/// it never reads the tree back, and the caller discards the result.
#[derive(Default)]
struct BoundedDom {
    dom: RcDom,
    nodes: Cell<usize>,
    exceeded: Cell<bool>,
}

impl BoundedDom {
    /// Count one more node; false once the document has too many.
    fn admit(&self) -> bool {
        self.admit_many(1)
    }

    /// Count `count` more nodes; false once the document has too many.
    fn admit_many(&self, count: usize) -> bool {
        let nodes = self.nodes.get().saturating_add(count);
        self.nodes.set(nodes);
        if nodes > MAX_NODES {
            self.exceeded.set(true);
        }
        !self.exceeded.get()
    }

    /// Whether a node inserted under `parent` stays within the depth bound.
    fn admit_under(&self, parent: &Handle) -> bool {
        if depth(parent) >= MAX_DEPTH {
            self.exceeded.set(true);
        }
        !self.exceeded.get()
    }
}

/// Ancestors of `node`, counted up to [`MAX_DEPTH`].
fn depth(node: &Handle) -> usize {
    let mut depth = 0;
    let mut current = parent(node);
    while let Some(node) = current
        && depth < MAX_DEPTH
    {
        depth += 1;
        current = parent(&node);
    }
    depth
}

fn parent(node: &Handle) -> Option<Handle> {
    let weak = node.parent.take();
    let parent = weak.as_ref().and_then(Weak::upgrade);
    node.parent.set(weak);
    parent
}

fn has_parent(node: &Handle) -> bool {
    let weak = node.parent.take();
    let has = weak.is_some();
    node.parent.set(weak);
    has
}

fn ends_with_text(node: &Handle) -> bool {
    node.children
        .borrow()
        .last()
        .is_some_and(|last| matches!(last.data, NodeData::Text { .. }))
}

impl TreeSink for BoundedDom {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }

    /// Parse errors are never read; collecting them would grow with the input.
    fn parse_error(&self, _message: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.dom.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        self.dom.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let attrs = if self.admit() { attrs } else { Vec::new() };
        self.dom.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        let text = if self.admit() {
            text
        } else {
            StrTendril::new()
        };
        self.dom.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        if self.admit() {
            self.dom.create_pi(target, data)
        } else {
            self.dom.create_pi(StrTendril::new(), StrTendril::new())
        }
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        let new_text = matches!(child, NodeOrText::AppendText(_)) && !ends_with_text(parent);
        if self.exceeded.get() || !self.admit_under(parent) || (new_text && !self.admit()) {
            return;
        }
        self.dom.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if has_parent(element) {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        if self.admit() {
            self.dom
                .append_doctype_to_document(name, public_id, system_id);
        }
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        self.dom.get_template_contents(target)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.dom.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.set_quirks_mode(mode);
    }

    /// Text inserted here may merge with a preceding text node; it is counted
    /// as a new node either way.
    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        if self.exceeded.get()
            || parent(sibling).is_some_and(|parent| !self.admit_under(&parent))
            || (matches!(child, NodeOrText::AppendText(_)) && !self.admit())
        {
            return;
        }
        self.dom.append_before_sibling(sibling, child);
    }

    /// A repeated `<html>` or `<body>` tag merges its attributes into the
    /// existing element. The DOM compares them with every attribute the
    /// element already has, so a merge counts both against the node bound.
    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if attrs.is_empty() || self.exceeded.get() {
            return;
        }
        let existing = match &target.data {
            NodeData::Element { attrs, .. } => attrs.borrow().len(),
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::Text { .. }
            | NodeData::Comment { .. }
            | NodeData::ProcessingInstruction { .. } => return,
        };
        if self.admit_many(existing.saturating_add(attrs.len())) {
            self.dom.add_attrs_if_missing(target, attrs);
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        if !self.exceeded.get() {
            self.dom.remove_from_parent(target);
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if !self.exceeded.get() {
            self.dom.reparent_children(node, new_parent);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        self.dom.is_mathml_annotation_xml_integration_point(handle)
    }

    // `maybe_clone_an_option_into_selectedcontent` keeps its default no-op:
    // `RcDom`'s version scans the whole `select` subtree for every `option`
    // and, in this release, never finds a target to clone into.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::rc::Rc;

    /// `width` formatting elements left open, then `runs` blocks of text: the
    /// parser recreates every open formatting element inside each block.
    fn reopened_formatting(width: usize, runs: usize) -> String {
        let mut html = String::from("<div>");
        for index in 0..width {
            write!(html, "<b i={index}>").unwrap();
        }
        html.push_str("</div>");
        html.push_str(&"<div>x</div>".repeat(runs));
        html
    }

    /// The parse outcome without the DOM itself.
    fn outcome(result: &Result<Handle, Error>) -> String {
        result
            .as_ref()
            .map_or_else(|error| format!("{error:?}"), |_| "parsed".to_owned())
    }

    #[test]
    fn reconstructed_formatting_is_refused_while_parsing() {
        // About 110,000 nodes at depth 100, then about 3,000 at depth 300.
        for (width, runs) in [(100, 1_100), (300, 10)] {
            let result = parse(&reopened_formatting(width, runs), &CancellationToken::new());
            assert!(
                matches!(result, Err(Error::LimitExceeded(_))),
                "{width}x{runs}: {}",
                outcome(&result)
            );
        }
    }

    /// The attributes of an element, as `name=value`.
    fn attributes(node: &Handle) -> Vec<String> {
        let NodeData::Element { attrs, .. } = &node.data else {
            panic!("not an element");
        };
        attrs
            .borrow()
            .iter()
            .map(|attr| format!("{}={}", attr.name.local, attr.value))
            .collect()
    }

    #[test]
    fn merged_attributes_count_against_the_node_limit() {
        // Each stray `<html>` merges its attribute into the root element, and
        // the DOM compares it with every attribute merged before.
        let html = (0..3_000).fold(String::new(), |mut html, index| {
            write!(html, "<html a{index}>").unwrap();
            html
        });
        let result = parse(&html, &CancellationToken::new());
        assert!(
            matches!(result, Err(Error::LimitExceeded(_))),
            "{}",
            outcome(&result)
        );
        // A few stray tags still merge their attributes.
        let tree = parse(
            "<html lang=en><body><p>x</p><body class=late><html lang=fr dir=ltr>",
            &CancellationToken::new(),
        )
        .unwrap();
        let root = Rc::clone(&tree.children.borrow()[0]);
        assert_eq!(attributes(&root), ["lang=en", "dir=ltr"]);
        let body = Rc::clone(&root.children.borrow()[1]);
        assert_eq!(attributes(&body), ["class=late"]);
    }

    #[test]
    fn cancellation_stops_parsing() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = parse(&"<p>text</p>".repeat(1_000), &cancel);
        assert!(
            matches!(result, Err(Error::Cancelled)),
            "{}",
            outcome(&result)
        );
    }

    #[test]
    fn chunked_parsing_matches_a_single_pass() {
        // Text that ends one byte before the first chunk boundary after `tag`.
        let filler = |tag: &str| "a".repeat(CHUNK_BYTES - tag.len() - 1);
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
            reopened_formatting(20, 50),
        ];
        let converter = htmd::HtmlToMarkdown::new();
        for html in documents {
            let tree = parse(&html, &CancellationToken::new()).unwrap();
            assert_eq!(
                converter.tree_to_markdown(&tree),
                converter.convert(&html).unwrap()
            );
        }
    }
}
