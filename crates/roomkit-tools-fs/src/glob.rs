use std::num::NonZeroUsize;
use std::path::Path;

use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::paths::{self, PathPage, Root, first_entry, workspace_path};

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
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: GlobLimits::default(),
        })
    }

    /// Set positive host-owned bounds.
    ///
    /// # Errors
    /// Returns invalid input if any bound is zero.
    pub fn with_limits(mut self, limits: GlobLimits) -> Result<Self, Error> {
        if [
            limits.max_matches,
            limits.max_output_bytes,
            limits.max_scan_entries,
            limits.max_depth,
        ]
        .contains(&0)
        {
            return Err(Error::InvalidInput("glob limits must be positive".into()));
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

    /// Find one page of paths; cancellation stops waiting for filesystem work.
    ///
    /// # Errors
    /// Returns invalid patterns or offsets, refusal outside the root,
    /// unsupported non-directories or non-UTF-8 names, I/O failures, or limits.
    pub async fn glob(
        &self,
        input: GlobInput,
        cancel: &CancellationToken,
    ) -> Result<Output<PathPage>, Error> {
        tokio::select! { biased;
            () = cancel.cancelled() => Err(Error::Cancelled),
            result = self.search(input) => result,
        }
    }

    async fn search(&self, input: GlobInput) -> Result<Output<PathPage>, Error> {
        let matcher = paths::pattern(&input.pattern)?;
        let path = self.root.resolve(&input.path).await?;
        let prefix = self.root.prefix(&path)?;
        let mut scan = paths::walk(
            &self.root,
            &path,
            self.limits.max_scan_entries,
            self.limits.max_depth,
            paths::match_depth(&input.pattern),
        )
        .await?;
        scan.entries.retain(|entry| {
            entry
                .path
                .strip_prefix(&prefix)
                .is_some_and(|relative| matcher.is_match(relative))
        });
        paths::page(
            self.root.relative(&path)?,
            scan,
            input.offset,
            input
                .limit
                .map_or(self.limits.max_matches, NonZeroUsize::get)
                .min(self.limits.max_matches),
            self.limits.max_output_bytes,
        )
    }
}
