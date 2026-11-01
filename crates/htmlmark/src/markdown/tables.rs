//! Pipe tables, layout tables and their cells.

use super::{Fragment, Markdown, Undo};
use crate::Error;
use crate::inline::Inline;
use crate::table::{self, Table};
use crate::tree::NodeId;

impl Markdown<'_> {
    /// A data table outside any other table's cell becomes a pipe table; other
    /// tables render as their content.
    pub(super) fn open_table(&mut self, node: NodeId) -> Result<Undo, Error> {
        self.boundary()?;
        let pipe = (self.flat == 0 && self.cells == 0)
            .then(|| table::columns(self.tree, node))
            .flatten()
            .map(Table::new);
        self.tables.push(pipe);
        Ok(Undo::Table)
    }

    pub(super) fn close_table(&mut self) -> Result<(), Error> {
        if let Some(Some(table)) = self.tables.pop()
            && let Some(caption) = table.caption
        {
            self.blocks.blank();
            self.blocks.line(&caption)?;
        }
        self.boundary()
    }

    pub(super) fn open_row(&mut self) -> Result<Undo, Error> {
        if matches!(self.tables.last(), Some(Some(_))) {
            return Ok(Undo::Row);
        }
        self.boundary()?;
        Ok(Undo::Block)
    }

    pub(super) fn close_row(&mut self) -> Result<(), Error> {
        let Some(Some(table)) = self.tables.last_mut() else {
            return Ok(());
        };
        for line in table.end_row() {
            self.blocks.line(&line)?;
        }
        Ok(())
    }

    /// A cell or caption of a pipe table is collected as inline content;
    /// elsewhere it is a block.
    pub(super) fn open_cell(&mut self, fragment: Fragment) -> Result<Undo, Error> {
        if matches!(fragment, Fragment::Cell) {
            self.cells += 1;
        }
        let Some(Some(table)) = self.tables.last_mut() else {
            self.boundary()?;
            return Ok(match fragment {
                Fragment::Cell => Undo::Cell,
                Fragment::Caption => Undo::Block,
            });
        };
        table.in_fragment = true;
        self.flat += 1;
        self.fragments.push(Inline::fragment());
        Ok(Undo::Fragment(fragment))
    }

    pub(super) fn close_fragment(&mut self, fragment: Fragment) -> Result<(), Error> {
        self.flat = self.flat.saturating_sub(1);
        let content = self
            .fragments
            .pop()
            .map(|mut inline| inline.take_line())
            .unwrap_or_default();
        let Some(Some(table)) = self.tables.last_mut() else {
            return Ok(());
        };
        table.in_fragment = false;
        match fragment {
            Fragment::Cell => {
                self.cells = self.cells.saturating_sub(1);
                table.cell(&content);
            }
            Fragment::Caption if content.is_empty() => {}
            Fragment::Caption if table.has_rows() => table.caption = Some(content),
            Fragment::Caption => {
                self.blocks.line(&content)?;
                self.blocks.blank();
            }
        }
        Ok(())
    }
}
