//! Table rules. htmd pads a pipe table only for a table with a `th` or
//! `thead` below it, outside nested tables, that is not itself inside a `td`
//! or `th`. Any other table is written as its plain content.

use super::kind::Kind;
use super::size::{Size, count};

/// Table rows gathered below a table, as a count and the widest cell bound of
/// each column.
#[derive(Debug, Default)]
pub(super) struct Rows {
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

/// What an element gathers for the table around it.
#[derive(Debug, Default)]
pub(super) struct Gathered {
    rows: Rows,
    /// Bounds of the cells directly inside, forming one row.
    cells: Vec<u64>,
    /// Captions directly inside, each on its own line.
    captions: Size,
    /// A `th` or `thead` below, outside nested tables.
    headers: bool,
    /// A `th` or `thead` below a table directly inside. htmd looks into such
    /// a table from this element only when this element is the table written.
    child_table_headers: bool,
}

impl Gathered {
    /// Whether a header cell below makes htmd consider a pipe table.
    pub(super) fn headers(&self) -> bool {
        self.headers
    }

    /// Rows below, plus the cells directly inside as one more row.
    pub(super) fn take_rows(&mut self) -> Rows {
        let mut rows = std::mem::take(&mut self.rows);
        if !self.cells.is_empty() {
            rows.merge(Rows {
                count: 1,
                widths: std::mem::take(&mut self.cells),
            });
        }
        rows
    }

    /// Record a finished child of `kind` with its bound, its rows, and whether
    /// it is or holds a header cell outside nested tables.
    pub(super) fn adopt(&mut self, kind: Kind, size: Size, rows: Rows, headers: bool) {
        if kind == Kind::Table {
            self.child_table_headers |= headers;
        } else {
            self.headers |= headers;
        }
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

    /// Bound of a table with this `content` and these `rows`. A table outside
    /// any cell with a header below it is the larger of its padded pipe form
    /// and its plain content, which htmd still writes when it finds no row:
    /// each row spans every column at its widest cell, plus a header separator
    /// row. Any other table is its plain content.
    pub(super) fn bound(&self, content: Size, rows: &Rows, in_cell: bool) -> Size {
        let mut size = content;
        if in_cell || !(self.headers || self.child_table_headers) {
            size.bytes = size.bytes.saturating_add(4);
            size.lines = size.lines.saturating_add(4);
            return size;
        }
        let line = rows.widths.iter().fold(2_u64, |line, width| {
            line.saturating_add(width.saturating_add(3))
        });
        let lines = rows.count.saturating_add(2);
        let pipe = lines
            .saturating_mul(line)
            .saturating_add(self.captions.bytes)
            .saturating_add(content.wide)
            .saturating_add(6);
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
}
