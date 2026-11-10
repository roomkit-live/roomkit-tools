use crate::{
    DocsContract, DocsListInput, DocsListResult, DocsReadInput, DocsReadResult, DocsSearchInput,
    DocsSearchResult, DocumentInfo,
};
use roomkit_tools_core::ToolError;
use std::collections::BTreeSet;
pub(crate) fn bounded(value: &str, max: usize) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(ToolError::InvalidInput(
            "expected nonempty text without NUL".into(),
        ));
    }
    if value.len() > max {
        return Err(ToolError::LimitExceeded(format!(
            "documentation field exceeds {max} bytes"
        )));
    }
    Ok(())
}
pub(crate) fn revision(value: &str) -> Result<(), ToolError> {
    if !value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }) {
        return Err(ToolError::InvalidInput(
            "invalid documentation revision".into(),
        ));
    }
    Ok(())
}
fn expected(value: Option<&str>) -> Result<(), ToolError> {
    value.map_or(Ok(()), revision)
}
fn limit(value: usize, max: usize) -> Result<(), ToolError> {
    if value == 0 || value > max {
        Err(ToolError::InvalidInput(format!("limit must be 1..{max}")))
    } else {
        Ok(())
    }
}
/// A page of results, as `page`, from an offset fs `list` accepts: none past
/// the last result, but for offset 1 when there is none.
fn results_page(
    offset: usize,
    count: usize,
    total: usize,
    next: Option<usize>,
    truncated: bool,
) -> Result<(), ToolError> {
    if offset > total && !(offset == 1 && total == 0) {
        return Err(ToolError::InvalidInput(
            "inconsistent documentation pagination".into(),
        ));
    }
    page(offset, count, total, next, truncated)
}
/// A page of `count` results from the one-based `offset`, of `total`; `next`
/// the one-based offset of the next page.
fn page(
    offset: usize,
    count: usize,
    total: usize,
    next: Option<usize>,
    truncated: bool,
) -> Result<(), ToolError> {
    let start = offset
        .checked_sub(1)
        .ok_or_else(|| ToolError::InvalidInput("offsets start at one".into()))?;
    let end = start
        .checked_add(count)
        .ok_or_else(|| ToolError::InvalidInput("invalid pagination".into()))?;
    if start > total
        || end > total
        || next != (end < total).then_some(end + 1)
        || truncated != next.is_some()
        || (count == 0 && start < total)
    {
        return Err(ToolError::InvalidInput(
            "inconsistent documentation pagination".into(),
        ));
    }
    Ok(())
}
impl DocsContract for DocsListInput {
    fn validate(&self) -> Result<(), ToolError> {
        limit(self.limit, 100)?;
        expected(self.expected_catalog_revision.as_deref())
    }
}
impl DocsContract for DocsSearchInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.query, 1024)?;
        limit(self.limit, 50)?;
        if self.query.split_whitespace().count() > 16 {
            return Err(ToolError::InvalidInput(
                "search accepts at most 16 terms".into(),
            ));
        }
        expected(self.expected_catalog_revision.as_deref())
    }
}
impl DocsContract for DocsReadInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.document_id, 512)?;
        limit(self.limit, 500)?;
        expected(self.expected_revision.as_deref())
    }
}
fn info(value: &DocumentInfo) -> Result<(), ToolError> {
    bounded(&value.document_id, 512)?;
    bounded(&value.title, 256)?;
    bounded(&value.source, 2048)?;
    if let Some(v) = &value.version {
        bounded(v, 128)?;
    }
    revision(&value.revision)
}
impl DocsContract for DocsListResult {
    fn text(&self) -> Option<String> {
        Some(crate::text::list(self))
    }
    fn validate(&self) -> Result<(), ToolError> {
        revision(&self.catalog_revision)?;
        if self.documents.len() > 100 {
            return Err(ToolError::LimitExceeded(
                "too many document summaries".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for doc in &self.documents {
            info(doc)?;
            if !ids.insert(&doc.document_id) {
                return Err(ToolError::InvalidInput("duplicate document ID".into()));
            }
        }
        results_page(
            self.offset,
            self.documents.len(),
            self.total_documents,
            self.next_offset,
            self.truncated,
        )
    }
}
impl DocsContract for DocsSearchResult {
    fn text(&self) -> Option<String> {
        Some(crate::text::search(self))
    }
    fn validate(&self) -> Result<(), ToolError> {
        revision(&self.catalog_revision)?;
        if self.matches.len() > 50 || self.matched_documents > self.total_documents {
            return Err(ToolError::InvalidInput(
                "invalid document match counts".into(),
            ));
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
                return Err(ToolError::InvalidInput("invalid document excerpt".into()));
            }
        }
        results_page(
            self.offset,
            self.matches.len(),
            self.matched_documents,
            self.next_offset,
            self.truncated,
        )
    }
}
impl DocsContract for DocsReadResult {
    fn text(&self) -> Option<String> {
        Some(crate::text::read(self))
    }
    fn validate(&self) -> Result<(), ToolError> {
        info(&self.document)?;
        revision(&self.catalog_revision)?;
        if self.next_offset == Some(0) {
            return Err(ToolError::InvalidInput("invalid next line".into()));
        }
        if self.offset == 0 {
            return Err(ToolError::InvalidInput("invalid source offset".into()));
        }
        if self.lines.len() > 500 {
            return Err(ToolError::LimitExceeded("too many document lines".into()));
        }
        for (index, line) in self.lines.iter().enumerate() {
            if Some(line.number) != self.offset.checked_add(index)
                || line.text.contains(['\n', '\0'])
            {
                return Err(ToolError::InvalidInput("invalid source line".into()));
            }
        }
        page(
            self.offset,
            self.lines.len(),
            self.document.line_count,
            self.next_offset,
            self.truncated,
        )
    }
}
