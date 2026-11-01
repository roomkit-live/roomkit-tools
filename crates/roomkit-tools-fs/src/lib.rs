//! Filesystem tools that a host selects and instantiates individually.
//!
//! Read, list and search beneath a canonical root without changing the
//! process's working directory. Tools are instantiated independently.
//! Each tool's limits type documents its bounds. Root checks assume a
//! host-controlled workspace; they do not provide OS sandboxing.

mod diff;
mod edit;
mod glob;
mod grep;
mod grep_scan;
mod list;
mod mutation;
mod patch;
mod paths;
mod read;
mod revision;
mod worker;
mod write;

pub use diff::{DiffHunk, DiffInput, DiffLimits, DiffLine, DiffLineKind, DiffResult, DiffTool};
pub use edit::{EditInput, EditLimits, EditResult, EditTool};
pub use glob::{GlobInput, GlobLimits, GlobTool};
pub use grep::{GrepInput, GrepLimits, GrepMatch, GrepPage, GrepTool};
pub use list::{ListInput, ListLimits, ListTool};
pub use mutation::MutationWorkspace;
pub use patch::{
    PatchApplied, PatchChange, PatchFailure, PatchInput, PatchLimits, PatchOperation, PatchPreview,
    PatchResult, PatchTool,
};
pub use paths::{PathEntry, PathKind, PathPage};
pub use read::{ReadInput, ReadLimits, ReadLine, ReadPage, ReadTool};
pub use write::{WriteInput, WriteLimits, WriteResult, WriteTool};
