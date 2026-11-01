//! [`Format::Markdown`](crate::Format::Markdown): walk events to Markdown.
//!
//! Each element's opening decides what it does and records how to undo it,
//! so its closing only replays that record. Headings and pipe-table cells
//! hold inline content only: inside them, block elements become spaces.

mod elements;
mod tables;

use html5ever::Attribute;

use crate::blocks::Blocks;
use crate::inline::Inline;
use crate::table::Table;
use crate::tree::{NodeId, Tree};
use crate::walk::{Event, walk};
use crate::{Error, Options};
use elements::language;

pub(crate) fn render(tree: &Tree, options: &Options<'_>) -> Result<String, Error> {
    let mut markdown = Markdown {
        tree,
        options,
        blocks: Blocks::new(options.max_output_bytes),
        paragraph: Inline::paragraph(),
        fragments: Vec::new(),
        flat: 0,
        lists: Vec::new(),
        tables: Vec::new(),
        cells: 0,
        undo: Vec::new(),
        ignored: 0,
        pre: None,
        code: None,
    };
    walk(tree, options, |event| markdown.event(event))?;
    markdown.flush()?;
    Ok(markdown.blocks.finish())
}

/// What an element's opening did, undone at its closing.
#[derive(Clone, Copy)]
enum Undo {
    Nothing,
    /// A block: a blank line before and after, or a space when flat.
    Block,
    Heading(Option<usize>),
    Quote(bool),
    List,
    Item(bool),
    /// A pipe-table row, written once its cells are collected.
    Row,
    Marker,
    Pre,
    Code,
    Table,
    /// A pipe-table cell or caption, collected as a fragment.
    Fragment(Fragment),
    /// A cell of a layout or flattened table.
    Cell,
    /// Content of a pipe table outside its cells.
    Ignored,
}

#[derive(Clone, Copy)]
enum Fragment {
    Cell,
    Caption,
}

struct List {
    ordered: bool,
    next: u64,
}

/// A `pre` element being collected.
struct Pre {
    text: String,
    language: Option<String>,
}

struct Markdown<'a> {
    tree: &'a Tree,
    options: &'a Options<'a>,
    blocks: Blocks,
    paragraph: Inline,
    /// Open pipe-table cells and captions; the innermost receives content.
    fragments: Vec<Inline>,
    /// Open headings and pipe-table cells.
    flat: usize,
    lists: Vec<List>,
    /// Open tables; `None` for a layout or flattened table.
    tables: Vec<Option<Table>>,
    /// Open `td` and `th` elements of every table.
    cells: usize,
    undo: Vec<Undo>,
    ignored: usize,
    pre: Option<Pre>,
    code: Option<String>,
}

impl Markdown<'_> {
    fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
        match event {
            Event::Open(name, attrs, node) => {
                let undo = self.open(name, attrs, node)?;
                self.undo.push(undo);
                Ok(())
            }
            Event::Close => match self.undo.pop() {
                Some(undo) => self.close(undo),
                None => Ok(()),
            },
            Event::Text(text) => {
                self.text(text);
                Ok(())
            }
        }
    }

    fn inline(&mut self) -> &mut Inline {
        self.fragments.last_mut().unwrap_or(&mut self.paragraph)
    }

    /// Whether text here belongs to a pipe table but none of its cells.
    fn between_cells(&self) -> bool {
        matches!(self.tables.last(), Some(Some(table)) if !table.in_fragment)
    }

    fn text(&mut self, text: &str) {
        if self.ignored > 0 || self.between_cells() {
            return;
        }
        if let Some(pre) = &mut self.pre {
            pre.text.push_str(text);
        } else if let Some(code) = &mut self.code {
            code.push_str(text);
        } else {
            self.inline().text(text);
        }
    }

    fn open(&mut self, name: &str, attrs: &[Attribute], node: NodeId) -> Result<Undo, Error> {
        let table_part = matches!(
            name,
            "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption"
        );
        if self.ignored > 0 || (self.between_cells() && !table_part) {
            self.ignored += 1;
            return Ok(Undo::Ignored);
        }
        if let Some(pre) = &mut self.pre {
            match name {
                "br" => pre.text.push('\n'),
                "code" if pre.language.is_none() => pre.language = language(attrs),
                _ => {}
            }
            return Ok(Undo::Nothing);
        }
        if let Some(code) = &mut self.code {
            if name == "br" {
                code.push(' ');
            }
            return Ok(Undo::Nothing);
        }
        self.open_element(name, attrs, node)
    }

    fn open_element(
        &mut self,
        name: &str,
        attrs: &[Attribute],
        node: NodeId,
    ) -> Result<Undo, Error> {
        Ok(match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.open_heading(name)?,
            "blockquote" => self.open_quote()?,
            "ul" | "ol" | "menu" | "dir" => self.open_list(name == "ol", attrs)?,
            "li" => self.open_item()?,
            "pre" | "listing" | "xmp" | "plaintext" => self.open_pre(attrs)?,
            "code" | "kbd" | "samp" | "tt" => {
                self.code = Some(String::new());
                Undo::Code
            }
            "b" | "strong" => self.marker("**"),
            "i" | "em" => self.marker("*"),
            "del" | "s" | "strike" => self.marker("~~"),
            "a" => self.open_link(attrs),
            "img" => {
                self.image(attrs);
                Undo::Nothing
            }
            "br" => {
                self.hard_break()?;
                Undo::Nothing
            }
            "hr" => {
                self.rule()?;
                Undo::Nothing
            }
            "table" => self.open_table(node)?,
            "tr" => self.open_row()?,
            "td" | "th" => self.open_cell(Fragment::Cell)?,
            "caption" => self.open_cell(Fragment::Caption)?,
            _ if is_block(name) => {
                self.boundary()?;
                Undo::Block
            }
            _ => Undo::Nothing,
        })
    }

    fn close(&mut self, undo: Undo) -> Result<(), Error> {
        match undo {
            Undo::Nothing => {}
            Undo::Block => self.boundary()?,
            Undo::Heading(level) => self.close_heading(level)?,
            Undo::Quote(opened) => self.close_container(opened, true)?,
            // Items of a list follow each other without blank lines.
            Undo::Item(opened) => self.close_container(opened, false)?,
            Undo::Row => self.close_row()?,
            Undo::List => {
                self.lists.pop();
                self.boundary()?;
            }
            Undo::Marker => self.inline().close(),
            Undo::Pre => self.close_pre()?,
            Undo::Code => self.close_code(),
            Undo::Table => self.close_table()?,
            Undo::Fragment(fragment) => self.close_fragment(fragment)?,
            Undo::Cell => {
                self.cells = self.cells.saturating_sub(1);
                self.boundary()?;
            }
            Undo::Ignored => self.ignored = self.ignored.saturating_sub(1),
        }
        Ok(())
    }

    /// Write the paragraph line, if any.
    fn flush(&mut self) -> Result<(), Error> {
        let line = self.paragraph.take_line();
        if line.is_empty() {
            return Ok(());
        }
        self.blocks.line(&line)
    }

    /// Separate blocks: a blank line, or a space inside inline-only content.
    fn boundary(&mut self) -> Result<(), Error> {
        if self.flat > 0 {
            self.inline().space();
            return Ok(());
        }
        self.flush()?;
        self.blocks.blank();
        Ok(())
    }
}

/// Elements that separate blocks of text.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "body"
            | "center"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "header"
            | "hgroup"
            | "html"
            | "legend"
            | "main"
            | "nav"
            | "optgroup"
            | "option"
            | "p"
            | "search"
            | "section"
            | "summary"
    )
}
