//! Incremental HTML parsing into a [`Tree`], bounded while it runs.
//!
//! The HTML parser recreates every open formatting element (`b`, `a`, `font`,
//! ...) inside each new block that follows them, so a short page can expand
//! into far more nodes than its source has tags. The sink counts nodes and
//! nesting as the parser creates and inserts them. Past a limit it stops
//! building the tree, and the input, fed in small chunks, is abandoned at the
//! next chunk boundary. The interrupt callback is checked between chunks too.
//!
//! Each repeated `<html>` or `<body>` tag merges its attributes into the
//! existing element after comparing them with every attribute already there,
//! so the sink counts a merge as that many nodes. The tokenizer also compares
//! each attribute of a tag with the earlier ones of the same tag before the
//! sink sees the element: a single tag with tens of thousands of attributes
//! takes seconds to tokenize, and only the interrupt callback stops that work,
//! at the next chunk boundary.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, ExpandedName, LocalName, Namespace, ParseOpts, QualName};

use crate::tree::{Data, NodeId, Tree};
use crate::{Error, Options};

/// Input fed to the parser between limit and interrupt checks.
pub(crate) const CHUNK_BYTES: usize = 1024;

/// Parse `source` in chunks of `chunk` bytes, refusing a document whose parse
/// creates more than `max_nodes` nodes or inserts one deeper than `max_depth`.
/// Depth is checked at insertion; nodes the parser moves later are checked
/// again by the renderer's walk.
pub(crate) fn parse(source: &str, options: &Options<'_>, chunk: usize) -> Result<Tree, Error> {
    let sink = Sink {
        tree: RefCell::new(Tree::new()),
        nodes: Cell::new(0),
        exceeded: Cell::new(false),
        max_nodes: options.max_nodes,
        max_depth: options.max_depth,
    };
    let mut parser = html5ever::parse_document(sink, ParseOpts::default());
    let mut rest = source;
    while !rest.is_empty() {
        options.check_interrupt()?;
        let (piece, tail) = rest
            .split_at_checked(rest.floor_char_boundary(chunk.max(4)))
            .unwrap_or((rest, ""));
        parser.process(StrTendril::from_slice(piece));
        if parser.tokenizer.sink.sink.exceeded.get() {
            return Err(Error::TooLarge);
        }
        rest = tail;
    }
    options.check_interrupt()?;
    let sink = parser.finish();
    if sink.exceeded.get() {
        return Err(Error::TooLarge);
    }
    Ok(sink.tree.into_inner())
}

/// A node and, for an element, its name: the parser reads names often and
/// keeps no borrow of the tree while doing so.
#[derive(Clone)]
pub(crate) struct Handle {
    id: NodeId,
    name: QualName,
}

fn unnamed(id: NodeId) -> Handle {
    Handle {
        id,
        name: QualName::new(None, Namespace::from(""), LocalName::from("")),
    }
}

/// Builds the [`Tree`], ignoring every change once a bound is exceeded.
///
/// Past a bound, new nodes are detached and carry no attributes or text. The
/// parser's own state stays consistent, since it never reads the tree back
/// beyond the names its handles carry, and the caller discards the result.
pub(crate) struct Sink {
    tree: RefCell<Tree>,
    nodes: Cell<usize>,
    exceeded: Cell<bool>,
    max_nodes: usize,
    max_depth: usize,
}

impl Sink {
    /// Count `count` more nodes; false once the document has too many.
    fn admit(&self, count: usize) -> bool {
        let nodes = self.nodes.get().saturating_add(count);
        self.nodes.set(nodes);
        if nodes > self.max_nodes {
            self.exceeded.set(true);
        }
        !self.exceeded.get()
    }

    /// Whether a node inserted under `parent` stays within the depth bound.
    fn admit_under(&self, parent: NodeId) -> bool {
        let tree = self.tree.borrow();
        let mut depth = 0;
        let mut current = tree.parent(parent);
        while let Some(node) = current
            && depth < self.max_depth
        {
            depth += 1;
            current = tree.parent(node);
        }
        if depth >= self.max_depth {
            self.exceeded.set(true);
        }
        !self.exceeded.get()
    }

    fn new_node(&self, data: Data) -> NodeId {
        self.tree.borrow_mut().push(data)
    }

    /// Append `text` to `node` when it is a text node.
    fn merge_text(&self, node: Option<NodeId>, text: &StrTendril) -> bool {
        let mut tree = self.tree.borrow_mut();
        match node.and_then(|node| tree.data_mut(node)) {
            Some(Data::Text(existing)) => {
                existing.push_tendril(text);
                true
            }
            Some(Data::Document | Data::Element { .. } | Data::Other) | None => false,
        }
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }

    /// Parse errors are never read; collecting them would grow with the input.
    fn parse_error(&self, _message: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        unnamed(Tree::DOCUMENT)
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        target.name.expanded()
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let attrs = if self.admit(1) { attrs } else { Vec::new() };
        let template = flags.template.then(|| {
            self.admit(1);
            self.new_node(Data::Document)
        });
        let id = self.new_node(Data::Element {
            name: name.clone(),
            attrs,
            template,
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        });
        Handle { id, name }
    }

    fn create_comment(&self, _text: StrTendril) -> Handle {
        self.admit(1);
        unnamed(self.new_node(Data::Other))
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Handle {
        self.admit(1);
        unnamed(self.new_node(Data::Other))
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        if self.exceeded.get() {
            return;
        }
        match child {
            NodeOrText::AppendNode(node) => {
                if self.admit_under(parent.id) {
                    self.tree.borrow_mut().append(parent.id, node.id);
                }
            }
            NodeOrText::AppendText(text) => {
                // Text joining the last text node is already at an admitted
                // depth; the parser appends a long text in many pieces.
                let last = self.tree.borrow().last_child(parent.id);
                if self.merge_text(last, &text) {
                    return;
                }
                if self.admit_under(parent.id) && self.admit(1) {
                    let node = self.new_node(Data::Text(text));
                    self.tree.borrow_mut().append(parent.id, node);
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if self.tree.borrow().parent(element.id).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        if self.admit(1) {
            let node = self.new_node(Data::Other);
            self.tree.borrow_mut().append(Tree::DOCUMENT, node);
        }
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        let template = match self.tree.borrow().data(target.id) {
            Some(Data::Element { template, .. }) => *template,
            Some(Data::Document | Data::Text(_) | Data::Other) | None => None,
        };
        // Only template elements are asked for; any other gets a detached,
        // never rendered node instead of a panic.
        unnamed(template.unwrap_or_else(|| self.new_node(Data::Document)))
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        x.id == y.id
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    /// Text inserted here may merge with a preceding text node; it is counted
    /// as a new node either way.
    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        let parent = self.tree.borrow().parent(sibling.id);
        if self.exceeded.get() || parent.is_some_and(|parent| !self.admit_under(parent)) {
            return;
        }
        let node = match child {
            NodeOrText::AppendNode(node) => node.id,
            NodeOrText::AppendText(text) => {
                let previous = self.tree.borrow().previous_sibling(sibling.id);
                if !self.admit(1) || self.merge_text(previous, &text) {
                    return;
                }
                self.new_node(Data::Text(text))
            }
        };
        self.tree.borrow_mut().insert_before(sibling.id, node);
    }

    /// A repeated `<html>` or `<body>` tag merges its attributes into the
    /// existing element; the merge counts both sets against the node bound.
    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if attrs.is_empty() || self.exceeded.get() {
            return;
        }
        let mut tree = self.tree.borrow_mut();
        let Some(Data::Element {
            attrs: existing, ..
        }) = tree.data_mut(target.id)
        else {
            return;
        };
        if !self.admit(existing.len().saturating_add(attrs.len())) {
            return;
        }
        let mut names: HashSet<QualName> = existing.iter().map(|attr| attr.name.clone()).collect();
        for attr in attrs {
            if names.insert(attr.name.clone()) {
                existing.push(attr);
            }
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        if !self.exceeded.get() {
            self.tree.borrow_mut().detach(target.id);
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if !self.exceeded.get() {
            self.tree
                .borrow_mut()
                .reparent_children(node.id, new_parent.id);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        matches!(
            self.tree.borrow().data(handle.id),
            Some(Data::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            })
        )
    }

    // `maybe_clone_an_option_into_selectedcontent` keeps its default no-op:
    // cloning an option into `selectedcontent` copies a whole subtree for every
    // selected option, and the copy adds nothing a reader of the page needs.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn options() -> Options<'static> {
        Options::default()
    }

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

    #[test]
    fn reconstructed_formatting_is_refused_while_parsing() {
        // About 110,000 nodes at depth 100, then about 3,000 at depth 300.
        for (width, runs) in [(100, 1_100), (300, 10)] {
            let result = parse(&reopened_formatting(width, runs), &options(), CHUNK_BYTES);
            assert!(matches!(result, Err(Error::TooLarge)), "{width}x{runs}");
        }
        assert!(parse(&reopened_formatting(20, 50), &options(), CHUNK_BYTES).is_ok());
    }

    /// The attributes of an element, as `name=value`.
    fn attributes(tree: &Tree, node: NodeId) -> Vec<String> {
        let Some(Data::Element { attrs, .. }) = tree.data(node) else {
            panic!("not an element");
        };
        attrs
            .iter()
            .map(|attr| format!("{}={}", attr.name.local, attr.value))
            .collect()
    }

    #[test]
    fn merged_attributes_count_against_the_node_limit() {
        // Each stray `<html>` merges its attribute into the root element, and
        // the merge compares it with every attribute merged before.
        let html = (0..3_000).fold(String::new(), |mut html, index| {
            write!(html, "<html a{index}>").unwrap();
            html
        });
        let result = parse(&html, &options(), CHUNK_BYTES);
        assert!(matches!(result, Err(Error::TooLarge)));
        // A few stray tags still merge their attributes.
        let tree = parse(
            "<html lang=en><body><p>x</p><body class=late><html lang=fr dir=ltr>",
            &options(),
            CHUNK_BYTES,
        )
        .unwrap();
        let root = tree.first_child(Tree::DOCUMENT).unwrap();
        assert_eq!(attributes(&tree, root), ["lang=en", "dir=ltr"]);
        let body = tree.next_sibling(tree.first_child(root).unwrap()).unwrap();
        assert_eq!(attributes(&tree, body), ["class=late"]);
    }

    #[test]
    fn the_interrupt_callback_stops_parsing() {
        let stop = || true;
        let mut options = options();
        options.interrupt = Some(&stop);
        let result = parse(&"<p>text</p>".repeat(1_000), &options, CHUNK_BYTES);
        assert!(matches!(result, Err(Error::Interrupted)));
    }

    #[test]
    fn templates_keep_their_content_outside_the_tree() {
        let tree = parse(
            "<template><p>inside</p></template><p>outside</p>",
            &options(),
            CHUNK_BYTES,
        )
        .unwrap();
        let mut texts = Vec::new();
        let mut stack = vec![Tree::DOCUMENT];
        while let Some(node) = stack.pop() {
            if let Some(text) = tree.text(node) {
                texts.push(text.to_owned());
            }
            let mut child = tree.last_child(node);
            while let Some(id) = child {
                stack.push(id);
                child = tree.previous_sibling(id);
            }
        }
        assert_eq!(texts, ["outside"]);
    }
}
