use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use roomkit_tools_core::{Error, Output, WorkspaceRoot};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

/// An entry's type, without following its symbolic link.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, including a broken link.
    Symlink,
    /// A device, socket, pipe or another special entry.
    Other,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "other",
        })
    }
}

/// One entry returned by listing or path search.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PathEntry {
    /// UTF-8 filename, without its parent directories.
    pub name: String,
    /// Path relative to the tool's workspace root, usable by `read` or `list`.
    pub path: String,
    /// Entry type; symbolic links are not followed when classifying entries.
    pub kind: PathKind,
}

/// A bounded page of directory entries or matching paths.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PathPage {
    /// Workspace-relative directory that was listed or searched; `.` is the root.
    pub path: String,
    /// Entries ordered lexicographically by their workspace-relative paths.
    pub entries: Vec<PathEntry>,
    /// Whether another page exists.
    pub truncated: bool,
    /// One-based entry offset to request next, or `None` at the end.
    pub next_offset: Option<usize>,
    /// Entries left out of the whole scan because their names are not UTF-8;
    /// a skipped directory is not searched.
    pub skipped_entries: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Root(WorkspaceRoot);

impl Root {
    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(crate) async fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let given = path.as_ref();
        let path = fs::canonicalize(given).await.map_err(|source| Error::Io {
            operation: "resolve the workspace root",
            source,
        })?;
        if !fs::metadata(&path)
            .await
            .map_err(|source| Error::Io {
                operation: "inspect the workspace root",
                source,
            })?
            .is_dir()
        {
            return Err(Error::InvalidInput(
                "workspace root must be a directory".into(),
            ));
        }
        let given = std::path::absolute(given).unwrap_or_else(|_| path.clone());
        Ok(Self(WorkspaceRoot::new(path, &given)))
    }

    /// Join a model-supplied path lexically; refuses escapes without I/O.
    pub(crate) fn join(&self, input: &str) -> Result<PathBuf, Error> {
        self.0.join(input)
    }

    pub(crate) async fn resolve(&self, input: &str) -> Result<PathBuf, Error> {
        let path = fs::canonicalize(self.join(input)?)
            .await
            .map_err(|source| Error::Io {
                operation: "resolve the path",
                source,
            })?;
        if !path.starts_with(self.path()) {
            return Err(Error::Refused("path is outside the workspace root".into()));
        }
        Ok(path)
    }

    /// Workspace-relative form of a resolved path; `.` names the root.
    pub(crate) fn relative(&self, path: &Path) -> Result<String, Error> {
        self.0.relative(path)
    }

    /// Prefix that a workspace-relative path below `path` starts with.
    pub(crate) fn prefix(&self, path: &Path) -> Result<String, Error> {
        let relative = self.relative(path)?;
        Ok(if relative == "." {
            String::new()
        } else {
            format!("{relative}/")
        })
    }
}

pub(crate) fn workspace_path() -> String {
    ".".into()
}
pub(crate) const fn first_entry() -> NonZeroUsize {
    NonZeroUsize::MIN
}

/// Entries found by a bounded scan.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) entries: Vec<PathEntry>,
    /// Entries skipped because their names are not UTF-8. They count toward
    /// the scan bound; a skipped directory's contents are not visited.
    pub(crate) skipped: usize,
}

impl Scan {
    fn visited(&self) -> usize {
        self.entries.len() + self.skipped
    }
}

/// List one directory. `maximum` bounds the entries visited, skipped included.
pub(crate) async fn directory_entries(
    root: &Root,
    path: &Path,
    maximum: usize,
) -> Result<Scan, Error> {
    let info = fs::metadata(path).await.map_err(|source| Error::Io {
        operation: "inspect the directory",
        source,
    })?;
    if !info.is_dir() {
        return Err(Error::Unsupported("expected a directory".into()));
    }
    let mut directory = fs::read_dir(path).await.map_err(|source| Error::Io {
        operation: "open the directory",
        source,
    })?;
    let mut scan = Scan::default();
    while let Some(entry) = directory.next_entry().await.map_err(|source| Error::Io {
        operation: "list the directory",
        source,
    })? {
        if scan.visited() >= maximum {
            return Err(Error::LimitExceeded(
                "directory scan exceeds max_scan_entries".into(),
            ));
        }
        if scan.visited() % 256 == 255 {
            tokio::task::yield_now().await;
        }
        let Ok(name) = entry.file_name().into_string() else {
            scan.skipped += 1;
            continue;
        };
        let kind = entry.file_type().await.map_err(|source| Error::Io {
            operation: "inspect the directory entry",
            source,
        })?;
        scan.entries.push(PathEntry {
            name,
            path: root.relative(&entry.path())?,
            kind: kind_of(kind),
        });
    }
    Ok(scan)
}

fn kind_of(kind: std::fs::FileType) -> PathKind {
    if kind.is_symlink() {
        PathKind::Symlink
    } else if kind.is_file() {
        PathKind::File
    } else if kind.is_dir() {
        PathKind::Directory
    } else {
        PathKind::Other
    }
}

/// Visit `path` recursively, without following symlinks, sorted by path.
///
/// Directories at level `match_depth` (the starting directory is level one)
/// are listed but not entered: their contents could not match. `None` scans
/// every level. The entry and depth bounds apply to what is visited.
pub(crate) async fn walk(
    root: &Root,
    path: &Path,
    maximum: usize,
    max_depth: usize,
    match_depth: Option<usize>,
) -> Result<Scan, Error> {
    let mut pending = vec![(path.to_path_buf(), 1)];
    let mut scan = Scan::default();
    while let Some((directory, depth)) = pending.pop() {
        if depth > max_depth {
            return Err(Error::LimitExceeded(
                "directory scan exceeds max_depth".into(),
            ));
        }
        let listed = directory_entries(root, &directory, maximum - scan.visited()).await?;
        scan.skipped += listed.skipped;
        for entry in listed.entries {
            if entry.kind == PathKind::Directory && match_depth.is_none_or(|limit| depth < limit) {
                pending.push((root.resolve(&entry.path).await?, depth + 1));
            }
            scan.entries.push(entry);
        }
    }
    scan.entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(scan)
}

/// Deepest relative path, in components, that a glob can match, or `None`
/// when unbounded. With literal separators, `*`, `?` and literals never match
/// `/`, but `**` crosses directories and a class such as `[!a]` can match it.
pub(crate) fn match_depth(pattern: &str) -> Option<usize> {
    if pattern.contains("**") || pattern.contains('[') {
        None
    } else {
        Some(pattern.matches('/').count() + 1)
    }
}

/// Compile a model-supplied glob. A one-pattern set is built instead of a
/// single matcher because only the set reports regex compilation failures
/// (for example deeply nested alternations) as errors rather than panicking.
pub(crate) fn pattern(pattern: &str) -> Result<globset::GlobSet, Error> {
    if pattern.is_empty() || pattern.len() > 4096 {
        return Err(Error::InvalidInput(
            "glob pattern must contain 1 to 4096 bytes".into(),
        ));
    }
    let invalid =
        |error: globset::Error| Error::InvalidInput(format!("invalid glob pattern: {error}"));
    let glob = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map_err(invalid)?;
    globset::GlobSetBuilder::new()
        .add(glob)
        .build()
        .map_err(invalid)
}

pub(crate) fn page(
    path: String,
    scan: Scan,
    offset: NonZeroUsize,
    limit: usize,
    bytes: usize,
) -> Result<Output<PathPage>, Error> {
    let Scan { entries, skipped } = scan;
    let offset = offset.get();
    if offset > entries.len() && !(offset == 1 && entries.is_empty()) {
        return Err(Error::InvalidInput(
            "offset is beyond the last entry".into(),
        ));
    }
    let mut selected = Vec::new();
    let mut text = String::new();
    let mut next_offset = None;
    for (index, entry) in entries.into_iter().enumerate().skip(offset - 1) {
        let number = index + 1;
        let rendered = format!("{number}: {} {:?}\n", entry.kind, entry.path);
        if selected.len() >= limit || rendered.len() > bytes - text.len() {
            if selected.is_empty() {
                return Err(Error::LimitExceeded(
                    "one entry exceeds max_output_bytes".into(),
                ));
            }
            next_offset = Some(number);
            break;
        }
        text.push_str(&rendered);
        selected.push(entry);
    }
    Ok(Output::new(
        PathPage {
            path,
            entries: selected,
            truncated: next_offset.is_some(),
            next_offset,
            skipped_entries: skipped,
        },
        text,
    ))
}
