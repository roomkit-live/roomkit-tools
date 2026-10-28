use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use roomkit_tools_core::{Error, Output};
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
    /// Resolved directory that was listed or searched.
    pub path: String,
    /// Entries ordered lexicographically by their workspace-relative paths.
    pub entries: Vec<PathEntry>,
    /// Whether another page exists.
    pub truncated: bool,
    /// One-based entry offset to request next, or `None` at the end.
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Root(PathBuf);

impl Root {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) async fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = fs::canonicalize(path).await.map_err(|source| Error::Io {
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
        Ok(Self(path))
    }

    pub(crate) async fn resolve(&self, input: &str) -> Result<PathBuf, Error> {
        if input.is_empty() {
            return Err(Error::InvalidInput("path must not be empty".into()));
        }
        let path = fs::canonicalize(self.0.join(input))
            .await
            .map_err(|source| Error::Io {
                operation: "resolve the path",
                source,
            })?;
        if !path.starts_with(&self.0) {
            return Err(Error::Refused("path is outside the workspace root".into()));
        }
        Ok(path)
    }

    pub(crate) fn relative(&self, path: &Path) -> Result<String, Error> {
        let relative = path
            .strip_prefix(&self.0)
            .map_err(|_| Error::Refused("path is outside the workspace root".into()))?;
        relative
            .to_str()
            .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
            .ok_or_else(|| Error::Unsupported("path is not UTF-8".into()))
    }
}

pub(crate) fn workspace_path() -> String {
    ".".into()
}
pub(crate) const fn first_entry() -> NonZeroUsize {
    NonZeroUsize::MIN
}

pub(crate) async fn directory_entries(
    root: &Root,
    path: &Path,
    maximum: usize,
) -> Result<Vec<PathEntry>, Error> {
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
    let mut entries = Vec::new();
    while let Some(entry) = directory.next_entry().await.map_err(|source| Error::Io {
        operation: "list the directory",
        source,
    })? {
        if entries.len() >= maximum {
            return Err(Error::LimitExceeded(
                "directory scan exceeds max_scan_entries".into(),
            ));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::Unsupported("filename is not UTF-8".into()))?;
        let kind = entry.file_type().await.map_err(|source| Error::Io {
            operation: "inspect the directory entry",
            source,
        })?;
        let kind = if kind.is_symlink() {
            PathKind::Symlink
        } else if kind.is_file() {
            PathKind::File
        } else if kind.is_dir() {
            PathKind::Directory
        } else {
            PathKind::Other
        };
        entries.push(PathEntry {
            name,
            path: root.relative(&entry.path())?,
            kind,
        });
        if entries.len() % 256 == 0 {
            tokio::task::yield_now().await;
        }
    }
    Ok(entries)
}

pub(crate) async fn walk(
    root: &Root,
    path: &Path,
    maximum: usize,
    max_depth: usize,
) -> Result<Vec<PathEntry>, Error> {
    let mut pending = vec![(path.to_path_buf(), 1)];
    let mut entries = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        if depth > max_depth {
            return Err(Error::LimitExceeded(
                "directory scan exceeds max_depth".into(),
            ));
        }
        for entry in directory_entries(root, &directory, maximum - entries.len()).await? {
            if entry.kind == PathKind::Directory {
                pending.push((root.resolve(&entry.path).await?, depth + 1));
            }
            entries.push(entry);
        }
    }
    entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

pub(crate) fn pattern(pattern: &str) -> Result<globset::GlobMatcher, Error> {
    if pattern.is_empty() || pattern.len() > 4096 {
        return Err(Error::InvalidInput(
            "glob pattern must contain 1 to 4096 bytes".into(),
        ));
    }
    globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| Error::InvalidInput(format!("invalid glob pattern: {error}")))
}

pub(crate) fn page(
    path: &Path,
    entries: Vec<PathEntry>,
    offset: NonZeroUsize,
    limit: usize,
    bytes: usize,
) -> Result<Output<PathPage>, Error> {
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
            path: path.display().to_string(),
            entries: selected,
            truncated: next_offset.is_some(),
            next_offset,
        },
        text,
    ))
}
