//! Markdown is authoritative. The index can be rebuilt after an interrupted write.
use super::{MemoryBackend, MemoryFuture, MemoryInput, MemoryNote, MemoryResult};
use crate::types::{MAX_NOTE, MAX_RESULTS, invalid, valid_id};
use roomkit_tools_core::ToolError;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// Markdown notes under a trusted directory, protected by a cross-process lock.
#[derive(Debug, Clone)]
pub struct FileMemory {
    root: PathBuf,
    max_bytes: usize,
}

fn io(source: std::io::Error) -> ToolError {
    ToolError::Io {
        operation: "access project memory",
        source,
    }
}
fn limit() -> ToolError {
    ToolError::LimitExceeded("Project memory is full; shorten or forget an existing note".into())
}
fn cancelled(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

impl FileMemory {
    /// Bind a directory and total byte quota, including the generated index.
    pub fn new(root: PathBuf, max_bytes: usize) -> Self {
        Self { root, max_bytes }
    }

    fn apply(
        &self,
        input: &MemoryInput,
        cancel: &CancellationToken,
    ) -> Result<MemoryResult, ToolError> {
        input.validate()?;
        cancelled(cancel)?;
        if !self.root.try_exists().map_err(io)? && input.action != "save" {
            return if input.action == "search" {
                Ok(MemoryResult { notes: vec![] })
            } else {
                Err(invalid("Unknown project note"))
            };
        }
        let mut directory = fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            directory.mode(0o700);
        }
        directory.create(&self.root).map_err(io)?;
        if fs::symlink_metadata(&self.root)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(invalid("Memory root must be a directory"));
        }
        let _lock = acquire(&self.root.join(".lock"), cancel)?;
        let mut rows = self.load(cancel)?;
        self.recover_index(&rows)?;
        let id = input.id.as_deref().unwrap_or_default();
        let current = rows.get(id);
        match input.action.as_str() {
            "search" => {
                let query = input.query.as_deref().unwrap_or_default().to_lowercase();
                let found = rows
                    .values()
                    .filter(|n| {
                        let text = format!(
                            "{} {} {}",
                            n.id,
                            n.title,
                            n.text.as_deref().unwrap_or_default()
                        )
                        .to_lowercase();
                        query.split_whitespace().all(|word| text.contains(word))
                    })
                    .cloned()
                    .map(metadata)
                    .collect();
                Ok(MemoryResult { notes: found })
            }
            "read" => Ok(MemoryResult {
                notes: vec![
                    current
                        .ok_or_else(|| invalid("Unknown project note"))?
                        .clone(),
                ],
            }),
            "save" | "forget" => self.mutate(input, &mut rows, cancel),
            _ => Err(invalid("Unknown memory action")),
        }
    }

    // The topic replacement and index replacement are individually atomic. A crash
    // between them leaves only the derived index stale, so repair it under the lock.
    fn recover_index(&self, rows: &BTreeMap<String, MemoryNote>) -> Result<(), ToolError> {
        let path = self.root.join("MEMORY.md");
        let expected = index(rows);
        match fs::symlink_metadata(&path) {
            Ok(entry) if !entry.is_file() => return Err(invalid("Invalid memory index")),
            Ok(entry)
                if entry.len() == expected.len() as u64
                    && fs::read(&path).map_err(io)? == expected.as_bytes() =>
            {
                return Ok(());
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(e)),
            Err(_) if rows.is_empty() => return Ok(()),
            _ => {}
        }
        atomic(&path, &expected)
    }

    fn load(&self, cancel: &CancellationToken) -> Result<BTreeMap<String, MemoryNote>, ToolError> {
        let mut rows = BTreeMap::new();
        // Per-note and count limits bound reads even when the configured quota shrinks.
        for item in fs::read_dir(&self.root).map_err(io)? {
            cancelled(cancel)?;
            let entry = item.map_err(io)?;
            let path = entry.path();
            if path.extension().is_none_or(|s| s != "md")
                || path.file_name().is_some_and(|s| s == "MEMORY.md")
            {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("Invalid Markdown filename"))?;
            if !valid_id(id) || !entry.file_type().map_err(io)?.is_file() {
                return Err(invalid("Invalid project memory file"));
            }
            let mut content = String::new();
            File::open(&path)
                .map_err(io)?
                .take((MAX_NOTE + 166) as u64)
                .read_to_string(&mut content)
                .map_err(io)?;
            let (title, text) = content
                .strip_prefix("# ")
                .and_then(|s| s.split_once("\n\n"))
                .ok_or_else(|| invalid("A note must have a Markdown heading and body"))?;
            let row = note(id, title, text);
            let read = MemoryInput {
                action: "read".into(),
                id: Some(id.into()),
                ..Default::default()
            };
            MemoryResult {
                notes: vec![row.clone()],
            }
            .validate(&read)?;
            if rows.len() >= MAX_RESULTS {
                return Err(limit());
            }
            rows.insert(id.into(), row);
        }
        Ok(rows)
    }

    fn mutate(
        &self,
        input: &MemoryInput,
        rows: &mut BTreeMap<String, MemoryNote>,
        cancel: &CancellationToken,
    ) -> Result<MemoryResult, ToolError> {
        let id = input.id.as_deref().unwrap_or_default();
        let old = rows.get(id);
        if old.map(|n| &n.revision) != input.revision.as_ref() {
            return Err(invalid(
                "Note changed or already exists; read its current revision",
            ));
        }
        let old_bytes = old.map_or(0, note_bytes);
        let old_index_bytes = index(rows).len();
        let path = self.root.join(format!("{id}.md"));
        let updated = if input.action == "save" {
            let row = note(
                id,
                input.title.as_deref().unwrap_or_default(),
                input.text.as_deref().unwrap_or_default(),
            );
            rows.insert(id.into(), row.clone());
            row
        } else {
            rows.remove(id)
                .ok_or_else(|| invalid("Unknown project note"))?
        };
        let index = index(rows);
        let total = rows.values().map(note_bytes).sum::<usize>() + index.len();
        let shrinking =
            rows.get(id).map_or(0, note_bytes) + index.len() < old_bytes + old_index_bytes;
        if (total > self.max_bytes && !shrinking) || rows.len() > MAX_RESULTS {
            return Err(limit());
        }
        cancelled(cancel)?;
        if input.action == "save" {
            atomic(
                &path,
                &format!(
                    "# {}\n\n{}",
                    updated.title,
                    updated.text.as_deref().unwrap_or_default()
                ),
            )?;
        } else {
            fs::remove_file(path).map_err(io)?;
        }
        atomic(&self.root.join("MEMORY.md"), &index)?;
        Ok(MemoryResult {
            notes: vec![metadata(updated)],
        })
    }
}

fn acquire(path: &Path, cancel: &CancellationToken) -> Result<File, ToolError> {
    if fs::symlink_metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err(invalid("Invalid memory lock file"));
    }
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io)?;
    let started = Instant::now();
    loop {
        cancelled(cancel)?;
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(invalid("Memory is busy; retry the operation"));
            }
            Err(fs::TryLockError::Error(e)) => return Err(io(e)),
        }
    }
}

fn atomic(path: &Path, content: &str) -> Result<(), ToolError> {
    let mut temp = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| invalid("Invalid memory path"))?,
    )
    .map_err(io)?;
    temp.write_all(content.as_bytes()).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(path).map_err(|e| io(e.error))?;
    Ok(())
}

fn index(rows: &BTreeMap<String, MemoryNote>) -> String {
    use std::fmt::Write as _;
    let mut text = String::from("# Project memory\n");
    for row in rows.values() {
        let _ = writeln!(text, "- {}.md — {}", row.id, row.title);
    }
    text
}
fn note(id: &str, title: &str, text: &str) -> MemoryNote {
    let revision = Sha256::digest(format!("# {title}\n\n{text}").as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        });
    MemoryNote {
        id: id.into(),
        title: title.into(),
        text: Some(text.into()),
        revision,
    }
}
fn note_bytes(note: &MemoryNote) -> usize {
    note.title.len() + 4 + note.text.as_ref().map_or(0, String::len)
}
fn metadata(mut row: MemoryNote) -> MemoryNote {
    row.text = None;
    row
}

impl MemoryBackend for FileMemory {
    fn execute<'a>(
        &'a self,
        input: &'a MemoryInput,
        cancel: &'a CancellationToken,
    ) -> MemoryFuture<'a> {
        let (store, input, token) = (self.clone(), input.clone(), cancel.child_token());
        Box::pin(async move {
            let guard = token.clone().drop_guard();
            let result = tokio::task::spawn_blocking(move || store.apply(&input, &token))
                .await
                .map_err(|_| invalid("Memory worker interrupted"))?;
            drop(guard);
            result
        })
    }
}
