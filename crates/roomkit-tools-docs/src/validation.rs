use crate::{
    DocsContract, DocsListInput, DocsListResult, DocsReadInput, DocsReadResult, DocsSearchInput,
    DocsSearchResult, DocumentInfo,
};
use roomkit_tools_core::Error;
use std::collections::BTreeSet;
pub(crate) fn bounded(value: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(Error::InvalidInput(
            "expected nonempty text without NUL".into(),
        ));
    }
    if value.len() > max {
        return Err(Error::LimitExceeded(format!(
            "documentation field exceeds {max} bytes"
        )));
    }
    Ok(())
}
pub(crate) fn revision(value: &str) -> Result<(), Error> {
    if !value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }) {
        return Err(Error::InvalidInput("invalid documentation revision".into()));
    }
    Ok(())
}
fn expected(value: Option<&str>) -> Result<(), Error> {
    value.map_or(Ok(()), revision)
}
fn limit(value: usize, max: usize) -> Result<(), Error> {
    if value == 0 || value > max {
        Err(Error::InvalidInput(format!("limit must be 1..{max}")))
    } else {
        Ok(())
    }
}
fn page(
    offset: usize,
    count: usize,
    total: usize,
    next: Option<usize>,
    truncated: bool,
) -> Result<(), Error> {
    let end = offset
        .checked_add(count)
        .ok_or_else(|| Error::InvalidInput("invalid pagination".into()))?;
    if offset > total
        || end > total
        || next != (end < total).then_some(end)
        || truncated != next.is_some()
        || (count == 0 && offset < total)
    {
        return Err(Error::InvalidInput(
            "inconsistent documentation pagination".into(),
        ));
    }
    Ok(())
}
impl DocsContract for DocsListInput {
    fn validate(&self) -> Result<(), Error> {
        limit(self.limit, 100)?;
        expected(self.expected_catalog_revision.as_deref())
    }
}
impl DocsContract for DocsSearchInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.query, 1024)?;
        limit(self.limit, 50)?;
        if self.query.split_whitespace().count() > 16 {
            return Err(Error::InvalidInput(
                "search accepts at most 16 terms".into(),
            ));
        }
        expected(self.expected_catalog_revision.as_deref())
    }
}
impl DocsContract for DocsReadInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.document_id, 512)?;
        if self.offset == 0 {
            return Err(Error::InvalidInput("offset starts at line one".into()));
        }
        limit(self.limit, 500)?;
        expected(self.expected_revision.as_deref())
    }
}
fn info(value: &DocumentInfo) -> Result<(), Error> {
    bounded(&value.document_id, 512)?;
    bounded(&value.title, 256)?;
    bounded(&value.source, 2048)?;
    if let Some(v) = &value.version {
        bounded(v, 128)?;
    }
    revision(&value.revision)
}
impl DocsContract for DocsListResult {
    fn validate(&self) -> Result<(), Error> {
        revision(&self.catalog_revision)?;
        if self.documents.len() > 100 {
            return Err(Error::LimitExceeded("too many document summaries".into()));
        }
        let mut ids = BTreeSet::new();
        for doc in &self.documents {
            info(doc)?;
            if !ids.insert(&doc.document_id) {
                return Err(Error::InvalidInput("duplicate document ID".into()));
            }
        }
        page(
            self.offset,
            self.documents.len(),
            self.total_documents,
            self.next_offset,
            self.truncated,
        )
    }
}
impl DocsContract for DocsSearchResult {
    fn validate(&self) -> Result<(), Error> {
        revision(&self.catalog_revision)?;
        if self.matches.len() > 50 || self.matched_documents > self.total_documents {
            return Err(Error::InvalidInput("invalid document match counts".into()));
        }
        let mut ids = BTreeSet::new();
        for hit in &self.matches {
            info(&hit.document)?;
            if !ids.insert(&hit.document.document_id)
                || hit
                    .line
                    .is_some_and(|n| n == 0 || n > hit.document.line_count)
                || hit.truncated != (hit.omitted_bytes > 0)
                || hit.text.len() > 1024
            {
                return Err(Error::InvalidInput("invalid document excerpt".into()));
            }
        }
        page(
            self.offset,
            self.matches.len(),
            self.matched_documents,
            self.next_offset,
            self.truncated,
        )
    }
}
impl DocsContract for DocsReadResult {
    fn validate(&self) -> Result<(), Error> {
        info(&self.document)?;
        revision(&self.catalog_revision)?;
        if self.next_offset == Some(0) {
            return Err(Error::InvalidInput("invalid next line".into()));
        }
        let start = self
            .offset
            .checked_sub(1)
            .ok_or_else(|| Error::InvalidInput("invalid source offset".into()))?;
        if self.lines.len() > 500 {
            return Err(Error::LimitExceeded("too many document lines".into()));
        }
        for (index, line) in self.lines.iter().enumerate() {
            if Some(line.number) != self.offset.checked_add(index)
                || line.text.contains(['\n', '\0'])
            {
                return Err(Error::InvalidInput("invalid source line".into()));
            }
        }
        page(
            start,
            self.lines.len(),
            self.document.line_count,
            self.next_offset.and_then(|v| v.checked_sub(1)),
            self.truncated,
        )
    }
}
