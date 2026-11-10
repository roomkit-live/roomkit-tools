//! What a model reads of each documentation result: lines, not the JSON of
//! `data`. Identifiers, titles and sources come from the host's documents
//! and are quoted; a document's lines are numbered as `read` numbers them.

use std::fmt::Write as _;

use roomkit_tools_core::append_facts;

use crate::types::{DocsListResult, DocsReadResult, DocsSearchResult, DocumentInfo};

/// A document's identity: what to pass to `docs_read` and cite.
fn document(info: &DocumentInfo) -> String {
    let mut text = format!(
        "{:?} {:?} ({} lines, revision {}",
        info.document_id, info.title, info.line_count, info.revision
    );
    if let Some(version) = &info.version {
        let _written = write!(text, ", version {version:?}");
    }
    let _written = write!(text, ", source {:?})", info.source);
    text
}

fn next(text: &mut String, next_offset: Option<usize>, catalog_revision: &str) {
    append_facts(
        text,
        [
            ("next_offset", next_offset.map(|offset| offset.to_string())),
            ("catalog_revision", Some(catalog_revision.to_owned())),
        ],
    );
}

/// A catalogue page, one document per line.
pub(crate) fn list(result: &DocsListResult) -> String {
    let mut text = format!("{} documents\n", result.total_documents);
    for (index, info) in result.documents.iter().enumerate() {
        let _written = writeln!(text, "{}. {}", result.offset + index, document(info));
    }
    if result.documents.is_empty() {
        text.push_str("no documents on this page\n");
    }
    next(&mut text, result.next_offset, &result.catalog_revision);
    text
}

/// Ranked matches, each document with its matching line.
pub(crate) fn search(result: &DocsSearchResult) -> String {
    let mut text = format!(
        "{} of {} documents match\n",
        result.matched_documents, result.total_documents
    );
    for (index, hit) in result.matches.iter().enumerate() {
        let _written = writeln!(
            text,
            "{}. {}",
            result.offset + index,
            document(&hit.document)
        );
        let clipped = if hit.truncated { " [clipped]" } else { "" };
        let _written = match hit.line {
            Some(line) => writeln!(text, "   {line}: {}{clipped}", hit.text),
            None => writeln!(text, "   (title match){clipped}"),
        };
    }
    next(&mut text, result.next_offset, &result.catalog_revision);
    text
}

/// A page of a document's lines, numbered.
pub(crate) fn read(result: &DocsReadResult) -> String {
    let mut text = document(&result.document);
    text.push('\n');
    for line in &result.lines {
        let _written = writeln!(text, "{}: {}", line.number, line.text);
    }
    if result.lines.is_empty() {
        text.push_str("no lines\n");
    }
    next(&mut text, result.next_offset, &result.catalog_revision);
    text
}
