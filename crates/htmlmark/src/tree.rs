//! The parsed document: every node in one vector, linked by index.
//!
//! Nodes are never freed while the document lives, so an index stays valid
//! once created. Children form a doubly linked list, which keeps insertion
//! before a sibling and removal constant-time whatever the number of children.

use html5ever::tendril::StrTendril;
use html5ever::{Attribute, QualName};

/// Index of a node in its [`Tree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeId(usize);

pub(crate) enum Data {
    Document,
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
        /// Content of a `template` element, kept outside the tree.
        template: Option<NodeId>,
        mathml_annotation_xml_integration_point: bool,
    },
    Text(StrTendril),
    /// Doctype, comment or processing instruction: never rendered.
    Other,
}

struct Node {
    data: Data,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    previous: Option<NodeId>,
    next: Option<NodeId>,
}

impl Node {
    fn new(data: Data) -> Self {
        Self {
            data,
            parent: None,
            first_child: None,
            last_child: None,
            previous: None,
            next: None,
        }
    }
}

pub(crate) struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    pub(crate) const DOCUMENT: NodeId = NodeId(0);

    pub(crate) fn new() -> Self {
        Self {
            nodes: vec![Node::new(Data::Document)],
        }
    }

    /// Add a detached node.
    pub(crate) fn push(&mut self, data: Data) -> NodeId {
        self.nodes.push(Node::new(data));
        NodeId(self.nodes.len() - 1)
    }

    fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0)
    }

    pub(crate) fn data(&self, id: NodeId) -> Option<&Data> {
        self.node(id).map(|node| &node.data)
    }

    pub(crate) fn data_mut(&mut self, id: NodeId) -> Option<&mut Data> {
        self.node_mut(id).map(|node| &mut node.data)
    }

    pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.parent
    }

    pub(crate) fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.first_child
    }

    pub(crate) fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.last_child
    }

    pub(crate) fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.previous
    }

    pub(crate) fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.next
    }

    /// Unlink `id` from its parent and siblings; its children stay with it.
    pub(crate) fn detach(&mut self, id: NodeId) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let (parent, previous, next) = (node.parent.take(), node.previous.take(), node.next.take());
        match previous.and_then(|previous| self.node_mut(previous)) {
            Some(previous) => previous.next = next,
            None => {
                if let Some(parent) = parent.and_then(|parent| self.node_mut(parent)) {
                    parent.first_child = next;
                }
            }
        }
        match next.and_then(|next| self.node_mut(next)) {
            Some(next) => next.previous = previous,
            None => {
                if let Some(parent) = parent.and_then(|parent| self.node_mut(parent)) {
                    parent.last_child = previous;
                }
            }
        }
    }

    /// Move `child` to the end of `parent`'s children.
    pub(crate) fn append(&mut self, parent: NodeId, child: NodeId) {
        if parent == child {
            return;
        }
        self.detach(child);
        let last = self.last_child(parent);
        if let Some(node) = self.node_mut(child) {
            node.parent = Some(parent);
            node.previous = last;
        }
        match last.and_then(|last| self.node_mut(last)) {
            Some(last) => last.next = Some(child),
            None => {
                if let Some(parent) = self.node_mut(parent) {
                    parent.first_child = Some(child);
                }
            }
        }
        if let Some(parent) = self.node_mut(parent) {
            parent.last_child = Some(child);
        }
    }

    /// Move `child` just before `sibling`, which must have a parent.
    pub(crate) fn insert_before(&mut self, sibling: NodeId, child: NodeId) {
        let Some(parent) = self.parent(sibling).filter(|_| sibling != child) else {
            return;
        };
        self.detach(child);
        let previous = self.previous_sibling(sibling);
        if let Some(node) = self.node_mut(child) {
            node.parent = Some(parent);
            node.previous = previous;
            node.next = Some(sibling);
        }
        if let Some(node) = self.node_mut(sibling) {
            node.previous = Some(child);
        }
        match previous.and_then(|previous| self.node_mut(previous)) {
            Some(previous) => previous.next = Some(child),
            None => {
                if let Some(parent) = self.node_mut(parent) {
                    parent.first_child = Some(child);
                }
            }
        }
    }

    /// Move every child of `from` to the end of `to`'s children, in order.
    pub(crate) fn reparent_children(&mut self, from: NodeId, to: NodeId) {
        while let Some(child) = self.first_child(from) {
            self.append(to, child);
        }
    }

    /// The local name of an HTML element, or `None` for other nodes.
    pub(crate) fn element(&self, id: NodeId) -> Option<(&str, &[Attribute])> {
        match self.data(id)? {
            Data::Element { name, attrs, .. } if name.ns == html5ever::ns!(html) => {
                Some((name.local.as_ref(), attrs))
            }
            Data::Element { .. } | Data::Document | Data::Text(_) | Data::Other => None,
        }
    }

    /// The local name and attributes of an element outside the HTML
    /// namespace.
    pub(crate) fn foreign(&self, id: NodeId) -> Option<(&str, &[Attribute])> {
        match self.data(id)? {
            Data::Element { name, attrs, .. } if name.ns != html5ever::ns!(html) => {
                Some((name.local.as_ref(), attrs))
            }
            Data::Element { .. } | Data::Document | Data::Text(_) | Data::Other => None,
        }
    }

    pub(crate) fn text(&self, id: NodeId) -> Option<&str> {
        match self.data(id)? {
            Data::Text(text) => Some(text),
            Data::Element { .. } | Data::Document | Data::Other => None,
        }
    }
}

/// The value of attribute `name` among `attrs`.
pub(crate) fn attribute<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| &*attr.name.local == name)
        .map(|attr| attr.value.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(tree: &Tree, parent: NodeId) -> Vec<NodeId> {
        let mut found = Vec::new();
        let mut child = tree.first_child(parent);
        while let Some(id) = child {
            assert_eq!(tree.parent(id), Some(parent));
            found.push(id);
            child = tree.next_sibling(id);
        }
        let mut backwards = Vec::new();
        let mut child = tree.last_child(parent);
        while let Some(id) = child {
            backwards.push(id);
            child = tree.previous_sibling(id);
        }
        backwards.reverse();
        assert_eq!(found, backwards, "links disagree");
        found
    }

    #[test]
    fn links_stay_consistent_through_moves() {
        let mut tree = Tree::new();
        let [a, b, c, d] = [0; 4].map(|_| tree.push(Data::Other));
        let root = Tree::DOCUMENT;
        tree.append(root, a);
        tree.append(root, b);
        tree.insert_before(a, c);
        assert_eq!(children(&tree, root), [c, a, b]);
        tree.insert_before(b, d);
        assert_eq!(children(&tree, root), [c, a, d, b]);
        tree.detach(a);
        assert_eq!(children(&tree, root), [c, d, b]);
        tree.append(d, c);
        tree.append(d, b);
        assert_eq!(children(&tree, root), [d]);
        assert_eq!(children(&tree, d), [c, b]);
        tree.reparent_children(d, a);
        assert_eq!(children(&tree, d), []);
        assert_eq!(children(&tree, a), [c, b]);
        // Moving a node before itself or a detached sibling changes nothing.
        tree.insert_before(d, a);
        assert_eq!(children(&tree, root), [a, d]);
        tree.insert_before(b, b);
        assert_eq!(children(&tree, a), [c, b]);
    }
}
