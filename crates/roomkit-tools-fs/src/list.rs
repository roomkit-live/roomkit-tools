use std::num::NonZeroUsize;
use std::path::Path;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::paths::{self, PathPage, Root, first_entry, workspace_path};

/// A non-recursive directory listing, including hidden entries.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Directory under the tool's root. Defaults to the root (`.`).
    #[serde(default = "workspace_path")]
    pub path: String,
    /// First entry, starting at one, in lexicographic path order.
    #[serde(default = "first_entry")]
    pub offset: NonZeroUsize,
    /// Requested entries, capped by the host. Omit for the host's limit.
    #[serde(default)]
    pub limit: Option<NonZeroUsize>,
}

/// The workspace root from its first entry, as Python's `list()`; set
/// `path`, `offset` or `limit` by struct update.
impl Default for ListInput {
    fn default() -> Self {
        Self {
            path: workspace_path(),
            offset: first_entry(),
            limit: None,
        }
    }
}

/// Host-owned bounds for a directory listing.
#[derive(Clone, Copy, Debug)]
pub struct ListLimits {
    /// Maximum entries returned per page. Default: 200.
    pub max_entries: usize,
    /// Maximum rendered bytes, including numbering and escaped paths. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Maximum entries scanned before sorting. A larger directory fails. Default: 10,000.
    pub max_scan_entries: usize,
}

impl Default for ListLimits {
    fn default() -> Self {
        Self {
            max_entries: 200,
            max_output_bytes: 50 * 1024,
            max_scan_entries: 10_000,
        }
    }
}

/// List a host-controlled directory beneath a canonical root.
///
/// Entries are sorted before pagination. Hidden entries are included, ignore
/// files are not consulted, and symlinks are listed without following targets.
/// Each call rescans the directory; offsets are not snapshots across mutations.
#[derive(Clone, Debug)]
pub struct ListTool {
    root: Root,
    limits: ListLimits,
}

impl ListTool {
    /// Construct a tool beneath an existing directory.
    ///
    /// # Errors
    /// Returns an I/O error for an unresolved root, or invalid input for a file.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: Root::new(root).await?,
            limits: ListLimits::default(),
        })
    }

    /// Set positive host-owned bounds.
    ///
    /// # Errors
    /// Returns invalid input if any bound is zero.
    pub fn with_limits(mut self, limits: ListLimits) -> Result<Self, ToolError> {
        if limits.max_entries == 0 || limits.max_output_bytes == 0 || limits.max_scan_entries == 0 {
            return Err(ToolError::InvalidInput(
                "list limits must be positive".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// The definition derived from the actual input and output types.
    pub fn definition() -> Definition {
        Definition::new::<ListInput, PathPage>(
            "list",
            "List a directory inside the workspace, without recursion. Includes hidden entries and entry types. Paths are relative to the workspace and can be passed to read. Sorted pages start at offset 1; continue with next_offset.",
        )
    }

    /// List one page; cancellation stops waiting for filesystem work.
    /// Entries whose names are not UTF-8 are skipped and counted in
    /// `skipped_entries`.
    ///
    /// # Errors
    /// Returns refusal outside the root, unsupported for a file, an I/O
    /// failure, invalid offset, or a scan/output limit failure.
    pub async fn list(
        &self,
        input: ListInput,
        cancel: &CancellationToken,
    ) -> Result<Output<PathPage>, ToolError> {
        tokio::select! { biased;
            () = cancel.cancelled() => Err(ToolError::Cancelled),
            result = self.list_directory(input) => result,
        }
    }

    async fn list_directory(&self, input: ListInput) -> Result<Output<PathPage>, ToolError> {
        let path = self.root.resolve(&input.path).await?;
        let mut scan =
            paths::directory_entries(&self.root, &path, self.limits.max_scan_entries).await?;
        scan.entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        paths::page(
            self.root.relative(&path)?,
            scan,
            input.offset,
            input
                .limit
                .map_or(self.limits.max_entries, NonZeroUsize::get)
                .min(self.limits.max_entries),
            self.limits.max_output_bytes,
        )
    }
}
