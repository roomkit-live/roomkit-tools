//! Pipe tables.
//!
//! A table with header cells (`th`) that is not inside another table's cell
//! becomes a GitHub-flavored pipe table; its first row is the header. Other
//! tables lay pages out, and their cells render as ordinary blocks. Cells are
//! not padded to a common width: padding can multiply a table's size, and a
//! pipe table reads the same without it.

use crate::tree::{NodeId, Tree};

/// The rows and cells of `table` that belong to it, not to nested tables.
fn rows(tree: &Tree, table: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    children(tree, table).flat_map(move |child| {
        let section = matches!(tree.element(child), Some(("thead" | "tbody" | "tfoot", _)));
        let rows: Box<dyn Iterator<Item = NodeId>> = if section {
            Box::new(children(tree, child))
        } else {
            Box::new(std::iter::once(child))
        };
        rows.filter(move |&row| matches!(tree.element(row), Some(("tr", _))))
    })
}

fn children(tree: &Tree, parent: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    std::iter::successors(tree.first_child(parent), move |&child| {
        tree.next_sibling(child)
    })
}

/// The column count of `table` when it is a data table: it has a header cell
/// and at least one row.
pub(crate) fn columns(tree: &Tree, table: NodeId) -> Option<usize> {
    let mut columns = 0;
    let mut header = false;
    for row in rows(tree, table) {
        let mut cells = 0;
        for cell in children(tree, row) {
            match tree.element(cell) {
                Some(("th", _)) => {
                    header = true;
                    cells += 1;
                }
                Some(("td", _)) => cells += 1,
                Some(_) | None => {}
            }
        }
        columns = columns.max(cells);
    }
    (header && columns > 0).then_some(columns)
}

/// A pipe table being written, one row at a time.
pub(crate) struct Table {
    columns: usize,
    rows: usize,
    cells: Vec<String>,
    /// A caption met after the first row, written below the table.
    pub(crate) caption: Option<String>,
    /// Whether one of its cells or its caption is open.
    pub(crate) in_fragment: bool,
}

impl Table {
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            columns,
            rows: 0,
            cells: Vec::new(),
            caption: None,
            in_fragment: false,
        }
    }

    pub(crate) fn has_rows(&self) -> bool {
        self.rows > 0
    }

    /// Add a cell's Markdown, escaping the pipes that would split it.
    pub(crate) fn cell(&mut self, markdown: &str) {
        self.cells.push(markdown.replace('|', "\\|"));
    }

    /// The lines of the finished row: the header row also gets the delimiter
    /// row. Missing cells are empty; there are never more than `columns`. A
    /// row without cells writes nothing.
    pub(crate) fn end_row(&mut self) -> Vec<String> {
        let mut cells = std::mem::take(&mut self.cells);
        if cells.is_empty() {
            return Vec::new();
        }
        cells.resize(self.columns, String::new());
        let mut lines = vec![format!("| {} |", cells.join(" | "))];
        if self.rows == 0 {
            lines.push(format!("|{}", " --- |".repeat(self.columns)));
        }
        self.rows += 1;
        lines
    }
}
