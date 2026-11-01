//! Document-order walk over the rendered part of a [`Tree`], without
//! recursion, enforcing the node and depth limits again: the parser can move
//! nodes after inserting them, and a walk that counts what it visits also ends
//! on any tree shape.

use html5ever::Attribute;

use crate::tree::{NodeId, Tree, attribute};
use crate::{Error, Options};

/// Elements left out with everything inside them.
const SKIPPED: &[&str] = &[
    "head", "script", "style", "noscript", "template", "iframe", "object", "embed",
];

/// Nodes rendered between two interrupt checks.
const INTERRUPT_EVERY: usize = 256;

#[derive(Clone, Copy)]
pub(crate) enum Event<'t> {
    /// An HTML element; its children and its [`Event::Close`] follow.
    Open(&'t str, &'t [Attribute], NodeId),
    Close,
    Text(&'t str),
}

/// Call `visit` for each rendered node in document order. Elements outside
/// the HTML namespace (SVG, MathML) are transparent: only their text shows.
pub(crate) fn walk(
    tree: &Tree,
    options: &Options<'_>,
    mut visit: impl FnMut(Event<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut nodes = 0_usize;
    let mut depth = 1;
    let mut current = tree.first_child(Tree::DOCUMENT);
    while let Some(node) = current {
        nodes += 1;
        if nodes > options.max_nodes || depth > options.max_depth {
            return Err(Error::TooLarge);
        }
        if nodes.is_multiple_of(INTERRUPT_EVERY) {
            options.check_interrupt()?;
        }
        let mut descend = false;
        if let Some(text) = tree.text(node) {
            visit(Event::Text(text))?;
        } else if let Some((name, attrs)) = tree.element(node) {
            if !skipped(name, attrs) {
                visit(Event::Open(name, attrs, node))?;
                descend = true;
            }
        } else {
            descend = is_foreign_element(tree, node);
        }
        if let Some(child) = tree.first_child(node).filter(|_| descend) {
            current = Some(child);
            depth += 1;
            continue;
        }
        if descend {
            close(tree, node, &mut visit)?;
        }
        current = next(tree, node, &mut depth, &mut visit)?;
    }
    Ok(())
}

/// The next node after `node` and its subtree, closing each element left on
/// the way up.
fn next(
    tree: &Tree,
    mut node: NodeId,
    depth: &mut usize,
    visit: &mut impl FnMut(Event<'_>) -> Result<(), Error>,
) -> Result<Option<NodeId>, Error> {
    loop {
        if let Some(sibling) = tree.next_sibling(node) {
            return Ok(Some(sibling));
        }
        match tree.parent(node) {
            Some(parent) if parent != Tree::DOCUMENT => {
                close(tree, parent, visit)?;
                *depth = depth.saturating_sub(1);
                node = parent;
            }
            Some(_) | None => return Ok(None),
        }
    }
}

fn close(
    tree: &Tree,
    node: NodeId,
    visit: &mut impl FnMut(Event<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    match tree.element(node) {
        Some(_) => visit(Event::Close),
        None => Ok(()),
    }
}

fn skipped(name: &str, attrs: &[Attribute]) -> bool {
    SKIPPED.contains(&name)
        || attribute(attrs, "hidden").is_some()
        || attribute(attrs, "aria-hidden") == Some("true")
}

/// An SVG or MathML element to walk into: its `style` and `script` children
/// and its hidden parts are skipped like HTML ones.
fn is_foreign_element(tree: &Tree, node: NodeId) -> bool {
    tree.foreign(node)
        .is_some_and(|(name, attrs)| !skipped(name, attrs))
}
