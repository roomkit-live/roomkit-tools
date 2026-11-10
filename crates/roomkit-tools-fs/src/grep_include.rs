//! Grep's `include` filter.

use globset::GlobSet;
use roomkit_tools_core::ToolError;

use crate::paths::{self, PathEntry};

/// Which files grep reads. A glob without `/` filters file names at any
/// depth, as `rg -g` does: `*.rs` keeps every Rust file under the search
/// path. A glob with `/` matches the path relative to the search path, where
/// `*` stops at a directory and `**` crosses them.
pub(crate) struct Include {
    glob: GlobSet,
    by_name: bool,
    depth: Option<usize>,
}

impl Include {
    /// Compile a model-supplied `include` glob.
    pub(crate) fn new(pattern: &str) -> Result<Self, ToolError> {
        let by_name = !pattern.contains('/');
        Ok(Self {
            glob: paths::pattern(pattern)?,
            by_name,
            depth: if by_name {
                None
            } else {
                paths::match_depth(pattern)
            },
        })
    }

    /// Deepest level a matching file can be at, or `None` when unbounded.
    pub(crate) fn depth(&self) -> Option<usize> {
        self.depth
    }

    /// Whether `file`, found below `prefix`, passes the filter.
    pub(crate) fn matches(&self, file: &PathEntry, prefix: &str) -> bool {
        let subject = if self.by_name {
            file.name.as_str()
        } else {
            file.path.strip_prefix(prefix).unwrap_or(&file.path)
        };
        self.glob.is_match(subject)
    }
}
