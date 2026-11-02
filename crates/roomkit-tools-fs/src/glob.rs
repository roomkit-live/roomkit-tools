use std::num::NonZeroUsize;
use std::path::Path;

use globset::GlobSet;
use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::paths::{self, PathPage, Root, Scan, first_entry, workspace_path};
use crate::worker;

/// Search paths relative to a directory using a glob pattern.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobInput {
    /// Pattern relative to `path`; `*` stays in a component, `**` crosses directories.
    pub pattern: String,
    /// Search directory beneath the workspace root. Defaults to `.`.
    #[serde(default = "workspace_path")]
    pub path: String,
    /// One-based match offset, in lexicographic workspace-relative path order.
    #[serde(default = "first_entry")]
    pub offset: NonZeroUsize,
    /// Requested matches, capped by the host.
    #[serde(default)]
    pub limit: Option<NonZeroUsize>,
}

impl GlobInput {
    /// Search the workspace root for `pattern`.
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            path: workspace_path(),
            offset: first_entry(),
            limit: None,
        }
    }
}

/// Host bounds for path discovery; exceeding a traversal bound fails the search.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct GlobLimits {
    /// Maximum matches per page. Default: 200.
    pub max_matches: usize,
    /// Maximum rendered bytes. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum entries visited, including nonmatches. Default: 10,000.
    pub max_scan_entries: usize,
    /// Maximum directory levels scanned; the starting directory is level one. Default: 32.
    pub max_depth: usize,
}

impl Default for GlobLimits {
    fn default() -> Self {
        Self {
            max_matches: 200,
            max_output_bytes: 50 * 1024,
            max_scan_entries: 10_000,
            max_depth: 32,
        }
    }
}

/// Recursive path search without following child symlinks.
///
/// Hidden entries are included; ignore files are not consulted. Files,
/// directories and symlinks can match. Each call performs a fresh bounded scan.
/// Pattern matching runs on a blocking worker, never on the async executor.
#[derive(Clone, Debug)]
pub struct GlobTool {
    root: Root,
    limits: GlobLimits,
}

impl GlobTool {
    /// Construct a tool beneath an existing directory.
    ///
    /// # Errors
    /// Returns an I/O error for an unresolved root, or invalid input for a file.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: GlobLimits::default(),
        })
    }

    /// Set positive host-owned bounds.
    ///
    /// # Errors
    /// Returns invalid input if any bound is zero.
    pub fn with_limits(mut self, limits: GlobLimits) -> Result<Self, ToolError> {
        if [
            limits.max_matches,
            limits.max_output_bytes,
            limits.max_scan_entries,
            limits.max_depth,
        ]
        .contains(&0)
        {
            return Err(ToolError::InvalidInput(
                "glob limits must be positive".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// The definition derived from actual input and output types.
    pub fn definition() -> Definition {
        Definition::new::<GlobInput, PathPage>(
            "glob",
            "Find file, directory and symlink paths using a glob relative to the search directory. Use **/*.txt for recursive text-file discovery. Hidden entries are included, ignore files are not applied, and child symlinks are not followed. Sorted pages start at offset 1; continue with next_offset.",
        )
    }

    /// Find one page of paths.
    ///
    /// Cancellation, or dropping the returned future, returns promptly; the
    /// blocking matching worker stops at its next check, before the next
    /// entry. Entries whose names are not UTF-8 are skipped and counted in
    /// `skipped_entries`, and a skipped directory is not searched.
    ///
    /// # Errors
    /// Returns invalid patterns or offsets, refusal outside the root,
    /// unsupported non-directories, I/O failures, or limits.
    pub async fn glob(
        &self,
        input: GlobInput,
        cancel: &CancellationToken,
    ) -> Result<Output<PathPage>, ToolError> {
        worker::cancellable(cancel, |cancel| self.search(input, cancel)).await
    }

    /// Walk on the async side, then match and page on a blocking worker that
    /// observes `cancel`.
    async fn search(
        &self,
        input: GlobInput,
        cancel: CancellationToken,
    ) -> Result<Output<PathPage>, ToolError> {
        let matcher = paths::pattern(&input.pattern)?;
        let path = self.root.resolve(&input.path).await?;
        let prefix = self.root.prefix(&path)?;
        let scan = paths::walk(
            &self.root,
            &path,
            self.limits.max_scan_entries,
            self.limits.max_depth,
            paths::match_depth(&input.pattern),
        )
        .await?;
        let searched = self.root.relative(&path)?;
        let limit = input
            .limit
            .map_or(self.limits.max_matches, NonZeroUsize::get)
            .min(self.limits.max_matches);
        let bytes = self.limits.max_output_bytes;
        worker::blocking("finish the path search", move || {
            let scan = matching(scan, &prefix, &matcher, &cancel)?;
            paths::page(searched, scan, input.offset, limit, bytes)
        })
        .await
    }
}

/// Keep the entries whose path below `prefix` matches; blocking, checking
/// `cancel` before each entry.
fn matching(
    scan: Scan,
    prefix: &str,
    matcher: &GlobSet,
    cancel: &CancellationToken,
) -> Result<Scan, ToolError> {
    let Scan { entries, skipped } = scan;
    let mut kept = Vec::new();
    for entry in entries {
        worker::check(cancel)?;
        if entry
            .path
            .strip_prefix(prefix)
            .is_some_and(|relative| matcher.is_match(relative))
        {
            kept.push(entry);
        }
    }
    Ok(Scan {
        entries: kept,
        skipped,
    })
}
