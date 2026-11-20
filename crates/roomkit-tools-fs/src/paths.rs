use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roomkit_tools_core::{Output, ToolError, WorkspaceRoot, append_facts};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::walk::Scan;
use crate::worker;

const RESOLVE: &str = "resolve the path";

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
    /// Whether the result is incomplete: another page exists, or a host
    /// bound stopped the scan, which the text names (`truncated_by`,
    /// `stopped_at`).
    pub truncated: bool,
    /// One-based entry offset to request next, or `None` at the end of what
    /// the scan found.
    pub next_offset: Option<usize>,
    /// Entries left out of the whole scan because their names are not UTF-8;
    /// a skipped directory is not searched.
    pub skipped_entries: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Root(Arc<WorkspaceRoot>);

impl Root {
    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(crate) async fn new(path: impl AsRef<Path>) -> Result<Self, ToolError> {
        let given = path.as_ref();
        let path = fs::canonicalize(given)
            .await
            .map_err(|source| ToolError::Io {
                operation: "resolve the workspace root",
                source,
            })?;
        if !fs::metadata(&path)
            .await
            .map_err(|source| ToolError::Io {
                operation: "inspect the workspace root",
                source,
            })?
            .is_dir()
        {
            return Err(ToolError::InvalidInput(
                "workspace root must be a directory".into(),
            ));
        }
        let given = std::path::absolute(given).unwrap_or_else(|_| path.clone());
        Ok(Self(Arc::new(WorkspaceRoot::new(path, &given))))
    }

    /// Join a model-supplied path lexically; refuses escapes without I/O.
    pub(crate) fn join(&self, input: &str) -> Result<PathBuf, ToolError> {
        self.0.join(input)
    }

    /// Join and canonicalize a model-supplied path, refusing it outside the
    /// root whether or not it exists; see [`WorkspaceRoot::resolve`].
    pub(crate) async fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
        let path = self.join(input)?;
        let root = self.clone();
        worker::blocking(RESOLVE, move || root.canonical(&path, RESOLVE)).await
    }

    /// [`Root::resolve`] for blocking workers.
    pub(crate) fn resolve_blocking(&self, input: &str) -> Result<PathBuf, ToolError> {
        self.canonical(&self.join(input)?, RESOLVE)
    }

    /// Canonicalize a joined path or one of its parents; blocking. See
    /// [`WorkspaceRoot::resolve`].
    pub(crate) fn canonical(
        &self,
        path: &Path,
        operation: &'static str,
    ) -> Result<PathBuf, ToolError> {
        self.0
            .resolve(path, operation, |path| std::fs::canonicalize(path))
    }

    /// The refusal of a joined path that names nothing; see
    /// [`WorkspaceRoot::missing`].
    pub(crate) fn missing(&self, path: &Path) -> ToolError {
        self.0.missing(path)
    }

    /// Workspace-relative form of a resolved path; `.` names the root.
    pub(crate) fn relative(&self, path: &Path) -> Result<String, ToolError> {
        self.0.relative(path)
    }

    /// Prefix that a workspace-relative path below `path` starts with.
    pub(crate) fn prefix(&self, path: &Path) -> Result<String, ToolError> {
        let relative = self.relative(path)?;
        Ok(if relative == "." {
            String::new()
        } else {
            format!("{relative}/")
        })
    }
}

/// Where a host bound left a scan incomplete.
#[derive(Clone, Debug)]
pub(crate) struct Stop {
    /// The bound's name, as the host sets it: `max_total_bytes`, for one.
    pub(crate) bound: &'static str,
    /// Workspace-relative path where the scan stopped.
    pub(crate) path: String,
}

impl Stop {
    /// The facts that tell a model the result is incomplete, and where.
    pub(crate) fn facts(stop: Option<&Self>) -> [(&'static str, Option<String>); 2] {
        [
            ("truncated_by", stop.map(|stop| stop.bound.to_owned())),
            ("stopped_at", stop.map(|stop| format!("{:?}", stop.path))),
        ]
    }
}

pub(crate) fn workspace_path() -> String {
    ".".into()
}
pub(crate) const fn first_entry() -> NonZeroUsize {
    NonZeroUsize::MIN
}

/// Compile a model-supplied glob. A one-pattern set is built instead of a
/// single matcher because only the set reports regex compilation failures
/// (for example deeply nested alternations) as errors rather than panicking.
pub(crate) fn pattern(pattern: &str) -> Result<globset::GlobSet, ToolError> {
    if pattern.is_empty() || pattern.len() > 4096 {
        return Err(ToolError::InvalidInput(
            "glob pattern must contain 1 to 4096 bytes".into(),
        ));
    }
    let invalid =
        |error: globset::Error| ToolError::InvalidInput(format!("invalid glob pattern: {error}"));
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
) -> Result<Output<PathPage>, ToolError> {
    let Scan {
        entries,
        skipped,
        stopped,
        ..
    } = scan;
    let offset = offset.get();
    // A stopped scan cannot tell whether entries lie past the offset.
    if stopped.is_none() && offset > entries.len() && !(offset == 1 && entries.is_empty()) {
        return Err(ToolError::InvalidInput(
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
                return Err(ToolError::LimitExceeded(
                    "one entry exceeds max_output_bytes".into(),
                ));
            }
            next_offset = Some(number);
            break;
        }
        text.push_str(&rendered);
        selected.push(entry);
    }
    if selected.is_empty() {
        text.push_str("no entries\n");
    }
    let [truncated_by, stopped_at] = Stop::facts(stopped.as_ref());
    append_facts(
        &mut text,
        [
            ("next_offset", next_offset.map(|next| next.to_string())),
            truncated_by,
            stopped_at,
            (
                "skipped_entries",
                (skipped > 0).then(|| skipped.to_string()),
            ),
        ],
    );
    Ok(Output::new(
        PathPage {
            path,
            entries: selected,
            truncated: next_offset.is_some() || stopped.is_some(),
            next_offset,
            skipped_entries: skipped,
        },
        text,
    ))
}
