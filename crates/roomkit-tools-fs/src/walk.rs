//! Bounded directory scans for list, glob and grep: one directory, or a tree
//! walked in path order. At a host bound a scan stops with what it found,
//! and says which bound stopped it and where.

use std::path::{Path, PathBuf};

use roomkit_tools_core::ToolError;
use tokio::fs;

use crate::ignores::Ignores;
use crate::paths::{PathEntry, PathKind, Root, Stop};

/// Entries found by a bounded scan.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) entries: Vec<PathEntry>,
    /// Entries skipped because their names are not UTF-8. They count toward
    /// the scan bound; a skipped directory's contents are not visited.
    pub(crate) skipped: usize,
    /// Entries the ignore rules left out. They count toward the scan bound;
    /// an ignored directory's contents are not visited.
    pub(crate) ignored: usize,
    /// The host bound that left the scan incomplete, and where.
    pub(crate) stopped: Option<Stop>,
}

impl Scan {
    fn visited(&self) -> usize {
        self.entries.len() + self.skipped + self.ignored
    }

    /// Add the entries of a directory at `depth`, sorted, that `rules` keep;
    /// return the subdirectories to enter, in path order. `enter` tells
    /// whether a directory at `depth` can hold a match; one past
    /// `max_depth` is not entered, and the first such is where the scan
    /// stopped.
    fn add(
        &mut self,
        entries: Vec<PathEntry>,
        rules: Option<&Ignores>,
        depth: usize,
        bounds: Bounds,
        enter: impl Fn(usize) -> bool,
    ) -> Vec<String> {
        let mut below = Vec::new();
        for entry in entries {
            if rules.is_some_and(|rules| rules.ignores(&entry)) {
                self.ignored += 1;
                continue;
            }
            if entry.kind == PathKind::Directory && enter(depth) {
                if depth < bounds.max_depth {
                    below.push(entry.path.clone());
                } else {
                    self.stopped.get_or_insert_with(|| Stop {
                        bound: "max_depth",
                        path: entry.path.clone(),
                    });
                }
            }
            self.entries.push(entry);
        }
        below
    }
}

/// The host bounds of a recursive scan.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// Entries visited, ignored and skipped ones included.
    pub(crate) max_scan_entries: usize,
    /// Directory levels listed; the directory searched is level one.
    pub(crate) max_depth: usize,
}

/// A directory a walk lists: its level, and the ignore rules it inherits.
pub(crate) struct Start {
    pub(crate) directory: PathBuf,
    pub(crate) depth: usize,
    pub(crate) ignores: Option<Ignores>,
}

/// List one directory. `maximum` bounds the entries visited, skipped
/// included: at the bound the listing stops with the entries it read, in
/// the order the filesystem gave them.
pub(crate) async fn directory_entries(
    root: &Root,
    path: &Path,
    maximum: usize,
) -> Result<Scan, ToolError> {
    let info = fs::metadata(path).await.map_err(|source| ToolError::Io {
        operation: "inspect the directory",
        source,
    })?;
    if !info.is_dir() {
        return Err(ToolError::UnsupportedResource(
            "expected a directory".into(),
        ));
    }
    let mut directory = fs::read_dir(path).await.map_err(|source| ToolError::Io {
        operation: "open the directory",
        source,
    })?;
    let mut scan = Scan::default();
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|source| ToolError::Io {
            operation: "list the directory",
            source,
        })?
    {
        if scan.visited() >= maximum {
            scan.stopped = Some(Stop {
                bound: "max_scan_entries",
                path: root.relative(path)?,
            });
            break;
        }
        if scan.visited() % 256 == 255 {
            tokio::task::yield_now().await;
        }
        let Ok(name) = entry.file_name().into_string() else {
            scan.skipped += 1;
            continue;
        };
        let kind = entry.file_type().await.map_err(|source| ToolError::Io {
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

/// Visit `start` recursively, without following symlinks, in path order,
/// and return the entries sorted by path.
///
/// Directories at level `match_depth` are listed but not entered: their
/// contents could not match. `None` scans every level. Each directory adds
/// its own ignore files to the rules it inherits, and the entries they
/// exclude are left out, unentered. A directory below `max_depth` is listed
/// but not entered; at `max_scan_entries` the walk ends. Either way the
/// scan returns what it found, with the bound and the path where it stopped,
/// the entries bound winning.
pub(crate) async fn walk(
    root: &Root,
    start: Start,
    bounds: Bounds,
    match_depth: Option<usize>,
) -> Result<Scan, ToolError> {
    let mut pending = vec![start];
    let mut scan = Scan::default();
    while let Some(next) = pending.pop() {
        let maximum = bounds.max_scan_entries.saturating_sub(scan.visited());
        let mut listed = directory_entries(root, &next.directory, maximum).await?;
        let rules = Ignores::within(next.ignores, &next.directory).await?;
        listed.entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        scan.skipped += listed.skipped;
        let enter = |depth| match_depth.is_none_or(|limit| depth < limit);
        let below = scan.add(listed.entries, rules.as_ref(), next.depth, bounds, enter);
        if listed.stopped.is_some() {
            scan.stopped = listed.stopped;
            break;
        }
        // Pushed last to first, so that the first is listed next.
        for path in below.into_iter().rev() {
            pending.push(Start {
                directory: root.resolve(&path).await?,
                depth: next.depth + 1,
                ignores: rules.clone(),
            });
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
