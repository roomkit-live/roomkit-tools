use std::path::Path;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{Workspace, mutation, revision};

/// A text file to create or replace.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteInput {
    /// File in the workspace.
    pub path: String,
    /// Written exactly, BOM and line endings included.
    pub content: String,
    /// Allow replacing an existing file.
    #[serde(default)]
    pub overwrite: bool,
    /// Revision from read/write/edit; the file must exist and match. Still needs `overwrite`.
    #[serde(default)]
    pub expected_revision: Option<String>,
}

impl WriteInput {
    /// Create a file without allowing replacement.
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            overwrite: false,
            expected_revision: None,
        }
    }
}

/// Host-owned input and destination bound.
#[derive(Clone, Copy, Debug)]
pub struct WriteLimits {
    /// Maximum bytes in both the new and an existing file. Default: 8 MiB.
    pub max_file_bytes: usize,
}

impl Default for WriteLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
        }
    }
}

/// A completed creation or replacement; content is not echoed to the model.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WriteResult {
    /// Workspace-relative path, usable by `read`.
    pub path: String,
    /// Whether the destination was created by this call.
    pub created: bool,
    /// Whether file contents changed or a new file was created.
    pub changed: bool,
    /// Resulting file size in UTF-8 bytes.
    pub bytes_written: usize,
    /// SHA-256 revision of the resulting raw bytes, including BOM and line endings.
    pub revision: String,
    /// Workspace-relative directories created by this call, outermost first.
    pub created_directories: Vec<String>,
}

/// Stage and commit complete UTF-8 text files beneath an explicit root.
#[derive(Clone, Debug)]
pub struct WriteTool {
    workspace: Workspace,
    limits: WriteLimits,
}

impl WriteTool {
    /// Construct a tool with its own mutation serialization scope.
    ///
    /// # Errors
    /// Returns invalid input or I/O for an invalid root.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self::from_workspace(Workspace::new(root).await?))
    }

    /// Share serialization with other tools using clones of this workspace.
    pub fn from_workspace(workspace: Workspace) -> Self {
        Self {
            workspace,
            limits: WriteLimits::default(),
        }
    }

    /// Set the positive host-owned file bound.
    ///
    /// # Errors
    /// Returns invalid input for a zero or unrepresentable bound.
    pub fn with_limits(mut self, limits: WriteLimits) -> Result<Self, ToolError> {
        mutation::validate_limit(limits.max_file_bytes)?;
        self.limits = limits;
        Ok(self)
    }

    /// Definition derived from the executed input and result types.
    pub fn definition() -> Definition {
        Definition::new::<WriteInput, WriteResult>(
            "write",
            "Create a UTF-8 text file with complete content; missing parent directories are created. Replacing an existing file needs overwrite: true; pass its revision from read as expected_revision to refuse a stale overwrite. Use edit for targeted changes.",
        )
    }

    /// Commit a file, with cancellation checked before the atomic commit.
    ///
    /// Dropping the future requests cancellation; an already-started commit
    /// may still finish. A waiting caller receives success after commit.
    /// Created parents can remain if later I/O or cancellation prevents commit.
    ///
    /// # Errors
    /// Returns invalid input, root/symlink/overwrite refusal, unsupported text,
    /// size limits, I/O failure or cancellation before commit.
    pub async fn write(
        &self,
        input: WriteInput,
        cancel: &CancellationToken,
    ) -> Result<Output<WriteResult>, ToolError> {
        let maximum = self.limits.max_file_bytes;
        self.workspace
            .run(cancel, move |root, cancel| {
                mutation::bounded_text(&input.content, maximum)?;
                let (path, parents) = mutation::write_destination(root, &input.path)?;
                let relative = root.relative(&path)?;
                let before = mutation::snapshot(&path, maximum)?;
                revision::check(
                    input.expected_revision.as_deref(),
                    before.as_ref().map(|snapshot| snapshot.bytes.as_slice()),
                )?;
                if before.is_some() && !input.overwrite {
                    return Err(ToolError::AccessRefused(
                        "destination already exists; overwrite must be true".into(),
                    ));
                }
                let changed = before
                    .as_ref()
                    .is_none_or(|before| before.bytes != input.content.as_bytes());
                mutation::check_cancel(cancel)?;
                let revision = revision::of(input.content.as_bytes());
                let created_directories = mutation::create_parents(root, &parents, cancel)?;
                if changed {
                    mutation::commit(
                        &path,
                        before.as_ref(),
                        input.content.as_bytes(),
                        maximum,
                        cancel,
                    )?;
                }
                let data = WriteResult {
                    path: relative,
                    created: before.is_none(),
                    changed,
                    bytes_written: input.content.len(),
                    revision,
                    created_directories,
                };
                let directories = if data.created_directories.is_empty() {
                    String::new()
                } else {
                    format!(", created_directories={:?}", data.created_directories)
                };
                let text = format!(
                    "write {:?}: created={}, changed={}, bytes={}{directories}, revision={}\n",
                    data.path, data.created, data.changed, data.bytes_written, data.revision
                );
                Ok(Output::new(data, text))
            })
            .await
    }
}
