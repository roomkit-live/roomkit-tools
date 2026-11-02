use crate::{
    DocsBackend, DocsContract, DocsFuture, DocsListInput, DocsListResult, DocsReadInput,
    DocsReadResult, DocsSearchInput, DocsSearchResult, DocumentInfo, DocumentLine, DocumentMatch,
    cancelled,
};
use roomkit_tools_core::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{OnceCell, Semaphore};
use tokio_util::sync::CancellationToken;

/// Host bounds on a shared immutable file snapshot. These are work/size limits,
/// not a hard peak-memory guarantee. Two workers may execute per backend.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FileDocsLimits {
    /// Maximum eligible documents, default 256, maximum 1024.
    pub max_documents: usize,
    /// Maximum original bytes per document, default 256 KiB, maximum 1 MiB.
    pub max_document_bytes: usize,
    /// Maximum original bytes in the snapshot, default 8 MiB, maximum 64 MiB.
    pub max_total_bytes: usize,
    /// Maximum scanned directory entries including ignored entries, default 4096.
    pub max_entries: usize,
    /// Maximum nested directories, default 16, maximum 64.
    pub max_depth: usize,
    /// Maximum logical lines across all documents, default 65536, maximum 131072.
    pub max_total_lines: usize,
    /// Serialized line-page budget, default 32 KiB, range 4096..65536.
    pub max_read_bytes: usize,
}
impl Default for FileDocsLimits {
    fn default() -> Self {
        Self {
            max_documents: 256,
            max_document_bytes: 256 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            max_entries: 4096,
            max_depth: 16,
            max_total_lines: 65536,
            max_read_bytes: 32 * 1024,
        }
    }
}
impl FileDocsLimits {
    /// Check host budgets before any I/O.
    /// # Errors
    /// Rejects zero or unsupported sizes.
    pub fn validate(&self) -> Result<(), ToolError> {
        if !(1..=1024).contains(&self.max_documents)
            || !(1..=1024 * 1024).contains(&self.max_document_bytes)
            || !(1..=64 * 1024 * 1024).contains(&self.max_total_bytes)
            || !(1..=65536).contains(&self.max_entries)
            || !(1..=64).contains(&self.max_depth)
            || !(1..=131_072).contains(&self.max_total_lines)
            || !(4096..=65536).contains(&self.max_read_bytes)
        {
            return Err(ToolError::InvalidInput(
                "invalid file documentation limits".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug)]
struct Document {
    info: DocumentInfo,
    lines: Vec<String>,
    title_lower: String,
    id_lower: String,
    body_lower: String,
}
#[derive(Debug)]
struct Snapshot {
    documents: Vec<Document>,
    revision: String,
}
#[derive(Debug)]
struct Inner {
    root: PathBuf,
    version: Option<String>,
    limits: FileDocsLimits,
    snapshot: OnceCell<Arc<Snapshot>>,
    slots: Arc<Semaphore>,
}
/// Read-only Markdown/text catalogue, loaded lazily once. Clones share its
/// snapshot; create another backend to refresh. Hidden names and symlinks are
/// excluded. The root must be host-controlled, not a hostile-filesystem sandbox.
#[derive(Clone, Debug)]
pub struct FileDocs {
    inner: Arc<Inner>,
}
impl FileDocs {
    /// Capture the root relative to the current directory; defer file I/O.
    /// Documents declare no version until [`Self::with_version`] sets one.
    /// # Errors
    /// Rejects invalid budgets or an unavailable current directory.
    pub fn new(root: impl AsRef<Path>, limits: FileDocsLimits) -> Result<Self, ToolError> {
        limits.validate()?;
        let path = root.as_ref();
        let root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| ToolError::Io {
                    operation: "capture documentation root",
                    source,
                })?
                .join(path)
        };
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                version: None,
                limits,
                snapshot: OnceCell::new(),
                slots: Arc::new(Semaphore::new(2)),
            }),
        })
    }
    /// Declare the documentation version every document reports, as
    /// Python's `FileDocs(root, version=...)`; set it before cloning.
    /// # Errors
    /// Rejects an empty version, one with NUL or over 128 bytes, and a backend
    /// already cloned.
    pub fn with_version(mut self, version: impl Into<String>) -> Result<Self, ToolError> {
        let version = version.into();
        crate::validation::bounded(&version, 128)?;
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            ToolError::AccessRefused("set the documentation version before cloning".into())
        })?;
        inner.version = Some(version);
        Ok(self)
    }
    async fn run<T, F>(&self, cancel: &CancellationToken, work: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Result<T, ToolError> + Send + 'static,
    {
        let token = cancel.child_token();
        let _guard = token.clone().drop_guard();
        let permit = tokio::select! {biased; ()=token.cancelled()=>return Err(ToolError::Cancelled), p=Arc::clone(&self.inner.slots).acquire_owned()=>p.map_err(|_|ToolError::UnsupportedResource("documentation workers closed".into()))?};
        let worker_token = token.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work(worker_token)
        });
        tokio::select! {biased; ()=token.cancelled()=>Err(ToolError::Cancelled),result=worker=>result.map_err(|_|ToolError::UnsupportedResource("documentation worker failed".into()))?}
    }
    async fn snapshot(&self, cancel: &CancellationToken) -> Result<Arc<Snapshot>, ToolError> {
        cancelled(cancel)?;
        let snapshot = self.inner.snapshot.get_or_try_init(|| async {
            let inner = Arc::clone(&self.inner);
            self.run(cancel, move |token| load(&inner, &token).map(Arc::new))
                .await
        });
        tokio::select! {biased; ()=cancel.cancelled()=>Err(ToolError::Cancelled),result=snapshot=>result.map(Arc::clone)}
    }
}
impl DocsBackend for FileDocs {
    fn list<'a>(
        &'a self,
        input: &'a DocsListInput,
        cancel: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsListResult> {
        Box::pin(async move {
            input.validate()?;
            let snapshot = self.snapshot(cancel).await?;
            check_revision(
                input.expected_catalog_revision.as_deref(),
                &snapshot.revision,
            )?;
            let total = snapshot.documents.len();
            let end = page_end(input.offset, input.limit, total)?;
            let documents = snapshot
                .documents
                .iter()
                .skip(input.offset)
                .take(end - input.offset)
                .map(|d| d.info.clone())
                .collect();
            cancelled(cancel)?;
            Ok(DocsListResult {
                documents,
                catalog_revision: snapshot.revision.clone(),
                total_documents: total,
                offset: input.offset,
                next_offset: (end < total).then_some(end),
                truncated: end < total,
            })
        })
    }
    fn search<'a>(
        &'a self,
        input: &'a DocsSearchInput,
        cancel: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsSearchResult> {
        Box::pin(async move {
            input.validate()?;
            let snapshot = self.snapshot(cancel).await?;
            let input = input.clone();
            self.run(cancel, move |token| search(&snapshot, &input, &token))
                .await
        })
    }
    fn read<'a>(
        &'a self,
        input: &'a DocsReadInput,
        cancel: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsReadResult> {
        Box::pin(async move {
            input.validate()?;
            let snapshot = self.snapshot(cancel).await?;
            let input = input.clone();
            let budget = self.inner.limits.max_read_bytes;
            self.run(cancel, move |token| read(&snapshot, &input, budget, &token))
                .await
        })
    }
}
fn check_revision(expected: Option<&str>, current: &str) -> Result<(), ToolError> {
    if expected.is_some_and(|e| e != current) {
        Err(ToolError::AccessRefused(
            "documentation revision changed; discover/search again".into(),
        ))
    } else {
        Ok(())
    }
}
fn page_end(offset: usize, limit: usize, total: usize) -> Result<usize, ToolError> {
    if offset > total {
        return Err(ToolError::InvalidInput(
            "offset is beyond available results".into(),
        ));
    }
    Ok(offset.saturating_add(limit).min(total))
}
fn io(operation: &'static str, source: std::io::Error) -> ToolError {
    ToolError::Io { operation, source }
}
fn paths(
    root: &Path,
    limits: &FileDocsLimits,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>, ToolError> {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    let mut entries = 0;
    while let Some((directory, depth)) = stack.pop() {
        cancelled(cancel)?;
        for entry in fs::read_dir(directory).map_err(|e| io("list documentation", e))? {
            cancelled(cancel)?;
            entries += 1;
            if entries > limits.max_entries {
                return Err(ToolError::LimitExceeded(
                    "documentation scan entry budget exceeded".into(),
                ));
            }
            let entry = entry.map_err(|e| io("inspect documentation entry", e))?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|e| io("inspect documentation type", e))?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth >= limits.max_depth {
                    return Err(ToolError::LimitExceeded(
                        "documentation directory depth exceeded".into(),
                    ));
                }
                stack.push((entry.path(), depth + 1));
                continue;
            }
            let path = entry.path();
            if !path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("txt"))
            {
                continue;
            }
            if !kind.is_file() {
                return Err(ToolError::UnsupportedResource(
                    "documentation accepts regular Markdown/text files only".into(),
                ));
            }
            if files.len() >= limits.max_documents {
                return Err(ToolError::LimitExceeded(
                    "documentation count exceeded".into(),
                ));
            }
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
fn bytes(
    path: &Path,
    root: &Path,
    max: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, ToolError> {
    let path = path.canonicalize().map_err(|e| io("resolve document", e))?;
    if !path.starts_with(root) {
        return Err(ToolError::AccessRefused(
            "document escaped configured root".into(),
        ));
    }
    let mut file = fs::File::open(path).map_err(|e| io("open document", e))?;
    let metadata = file.metadata().map_err(|e| io("inspect document", e))?;
    if !metadata.is_file() {
        return Err(ToolError::UnsupportedResource(
            "documentation accepts regular files only".into(),
        ));
    }
    if metadata.len() > max as u64 {
        return Err(ToolError::LimitExceeded(
            "document byte budget exceeded".into(),
        ));
    }
    let mut result = Vec::new();
    let mut buffer = [0; 16384];
    loop {
        cancelled(cancel)?;
        let count = file.read(&mut buffer).map_err(|e| io("read document", e))?;
        if count == 0 {
            break;
        }
        if result.len() + count > max {
            return Err(ToolError::LimitExceeded(
                "document byte budget exceeded".into(),
            ));
        }
        result.extend(buffer.iter().take(count));
    }
    Ok(result)
}
fn clipped(text: &str, max: usize) -> String {
    text.chars()
        .scan(0usize, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= max).then_some(c)
        })
        .collect()
}
fn load(inner: &Inner, cancel: &CancellationToken) -> Result<Snapshot, ToolError> {
    let root = inner
        .root
        .canonicalize()
        .map_err(|e| io("resolve documentation root", e))?;
    if !root.is_dir() {
        return Err(ToolError::UnsupportedResource(
            "documentation root must be a directory".into(),
        ));
    }
    let mut documents = Vec::new();
    let mut total = 0;
    let mut line_count = 0;
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(&inner.version).map_err(|e| ToolError::InvalidInput(e.to_string()))?,
    );
    for path in paths(&root, &inner.limits, cancel)? {
        cancelled(cancel)?;
        let id = path
            .strip_prefix(&root)
            .ok()
            .and_then(|p| p.to_str())
            .ok_or_else(|| {
                ToolError::UnsupportedResource("document IDs require UTF-8 paths".into())
            })?
            .replace(std::path::MAIN_SEPARATOR, "/");
        crate::validation::bounded(&id, 512)?;
        let data = bytes(&path, &root, inner.limits.max_document_bytes, cancel)?;
        total += data.len();
        if total > inner.limits.max_total_bytes {
            return Err(ToolError::LimitExceeded(
                "documentation catalogue byte budget exceeded".into(),
            ));
        }
        let revision = roomkit_tools_core::revision::sha256(&Sha256::digest(&data));
        let text = String::from_utf8(data).map_err(|_| {
            ToolError::UnsupportedResource("documentation must be UTF-8 text".into())
        })?;
        if text.contains('\0') {
            return Err(ToolError::UnsupportedResource(
                "binary documentation is unsupported".into(),
            ));
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let count = text.lines().count();
        line_count += count;
        if line_count > inner.limits.max_total_lines {
            return Err(ToolError::LimitExceeded(
                "documentation catalogue line budget exceeded".into(),
            ));
        }
        let title = clipped(
            text.lines()
                .find_map(|l| l.strip_prefix("# ").map(str::trim))
                .filter(|s| !s.is_empty())
                .unwrap_or(&id),
            256,
        );
        let info = DocumentInfo {
            document_id: id.clone(),
            title: title.clone(),
            source: id.clone(),
            version: inner.version.clone(),
            revision,
            line_count: count,
        };
        hash.update(serde_json::to_vec(&info).map_err(|e| ToolError::InvalidInput(e.to_string()))?);
        documents.push(Document {
            info,
            lines: text.lines().map(str::to_owned).collect(),
            title_lower: title.to_lowercase(),
            id_lower: id.to_lowercase(),
            body_lower: text.to_lowercase(),
        });
    }
    cancelled(cancel)?;
    Ok(Snapshot {
        documents,
        revision: roomkit_tools_core::revision::sha256(&hash.finalize()),
    })
}
fn search(
    snapshot: &Snapshot,
    input: &DocsSearchInput,
    cancel: &CancellationToken,
) -> Result<DocsSearchResult, ToolError> {
    check_revision(
        input.expected_catalog_revision.as_deref(),
        &snapshot.revision,
    )?;
    let terms: BTreeSet<String> = input
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut ranked = Vec::new();
    for doc in &snapshot.documents {
        cancelled(cancel)?;
        let mut score = 0;
        for term in &terms {
            if doc.title_lower.contains(term) {
                score += 8;
            } else if doc.id_lower.contains(term) {
                score += 4;
            } else if doc.body_lower.contains(term) {
                score += 1;
            } else {
                score = 0;
                break;
            }
        }
        if score > 0 {
            ranked.push((score, doc));
        }
    }
    ranked.sort_by(|(a, da), (b, db)| {
        b.cmp(a)
            .then_with(|| da.info.document_id.cmp(&db.info.document_id))
    });
    let total = ranked.len();
    let end = page_end(input.offset, input.limit, total)?;
    let mut matches = Vec::new();
    for (_, doc) in ranked.iter().skip(input.offset).take(end - input.offset) {
        cancelled(cancel)?;
        let line = doc.lines.iter().enumerate().find(|(_, line)| {
            let lower = line.to_lowercase();
            terms.iter().any(|t| lower.contains(t))
        });
        let (number, text) = line.map_or((None, doc.info.title.as_str()), |(n, text)| {
            (Some(n + 1), text.as_str())
        });
        let excerpt = clipped(text, 1024);
        let omitted_bytes = text.len() - excerpt.len();
        matches.push(DocumentMatch {
            document: doc.info.clone(),
            line: number,
            text: excerpt,
            omitted_bytes,
            truncated: omitted_bytes > 0,
        });
    }
    Ok(DocsSearchResult {
        matches,
        catalog_revision: snapshot.revision.clone(),
        total_documents: snapshot.documents.len(),
        matched_documents: total,
        offset: input.offset,
        next_offset: (end < total).then_some(end),
        truncated: end < total,
    })
}
fn read(
    snapshot: &Snapshot,
    input: &DocsReadInput,
    budget: usize,
    cancel: &CancellationToken,
) -> Result<DocsReadResult, ToolError> {
    let doc = snapshot
        .documents
        .iter()
        .find(|d| d.info.document_id == input.document_id)
        .ok_or_else(|| ToolError::AccessRefused("unknown document in this catalogue".into()))?;
    check_revision(input.expected_revision.as_deref(), &doc.info.revision)?;
    let start = input.offset - 1;
    let end = page_end(start, input.limit, doc.lines.len())?;
    let mut size = serde_json::to_vec(&doc.info)
        .map_err(|e| ToolError::InvalidInput(e.to_string()))?
        .len()
        + 512;
    let mut lines = Vec::new();
    for (index, text) in doc.lines.iter().enumerate().take(end).skip(start) {
        cancelled(cancel)?;
        let line = DocumentLine {
            number: index + 1,
            text: text.clone(),
        };
        let bytes = serde_json::to_vec(&line)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?
            .len()
            + 1;
        if size + bytes > budget {
            if lines.is_empty() {
                return Err(ToolError::LimitExceeded(
                    "document line cannot fit the read budget".into(),
                ));
            }
            break;
        }
        size += bytes;
        lines.push(line);
    }
    let next = start + lines.len();
    Ok(DocsReadResult {
        document: doc.info.clone(),
        catalog_revision: snapshot.revision.clone(),
        lines,
        offset: input.offset,
        next_offset: (next < doc.lines.len()).then_some(next + 1),
        truncated: next < doc.lines.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_worker_retains_its_slot_until_native_work_finishes() {
        let root = tempfile::tempdir().unwrap();
        let backend = FileDocs::new(root.path(), FileDocsLimits::default()).unwrap();
        let worker = backend.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let call = tokio::spawn(async move {
            worker
                .run(&token, move |_| {
                    entered.send(()).unwrap();
                    gate.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started.await.unwrap();
        cancel.cancel();
        assert!(matches!(call.await.unwrap(), Err(ToolError::Cancelled)));
        assert_eq!(backend.inner.slots.available_permits(), 1);
        release.send(()).unwrap();
        let _all = backend.inner.slots.acquire_many(2).await.unwrap();
        assert_eq!(backend.inner.slots.available_permits(), 0);
    }
}
