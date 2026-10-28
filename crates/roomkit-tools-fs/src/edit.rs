use std::path::Path;

use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{MutationWorkspace, mutation, revision};

/// Replace an exact, nonempty piece of a UTF-8 text file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    /// Existing regular file beneath the configured root.
    pub path: String,
    /// Exact text to match, including whitespace and line endings. Must not be empty.
    pub old_text: String,
    /// Replacement text; an empty string removes the matched text, not the file.
    pub new_text: String,
    /// Replace every non-overlapping occurrence. Defaults to false (exactly one).
    #[serde(default)]
    pub replace_all: bool,
    /// Optional content revision returned by read/write/edit. Refuses a changed file.
    #[serde(default)]
    pub expected_revision: Option<String>,
}

impl EditInput {
    /// Replace exactly one occurrence.
    pub fn new(
        path: impl Into<String>,
        old_text: impl Into<String>,
        new_text: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: false,
            expected_revision: None,
        }
    }
}

/// Host-owned source, argument and resulting file bound.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EditLimits {
    /// Maximum source, each text argument and resulting file bytes. Default: 8 MiB.
    pub max_file_bytes: usize,
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
        }
    }
}

/// Outcome of a completed exact-text edit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EditResult {
    /// Workspace-relative path, usable by `read`.
    pub path: String,
    /// Number of matched, non-overlapping occurrences.
    pub replacements: usize,
    /// Whether file bytes changed; false when old and new text are identical.
    pub changed: bool,
    /// File bytes before the edit.
    pub bytes_before: usize,
    /// File bytes after the edit.
    pub bytes_after: usize,
    /// SHA-256 revision of the resulting raw bytes, including BOM and line endings.
    pub revision: String,
}

/// Exact text replacement preserving all bytes outside matched spans.
#[derive(Clone, Debug)]
pub struct EditTool {
    workspace: MutationWorkspace,
    limits: EditLimits,
}

impl EditTool {
    /// Construct a tool with its own mutation serialization scope.
    ///
    /// # Errors
    /// Returns invalid input or I/O for an invalid root.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self::from_workspace(MutationWorkspace::new(root).await?))
    }

    /// Share serialization with other tools using clones of this workspace.
    pub fn from_workspace(workspace: MutationWorkspace) -> Self {
        Self {
            workspace,
            limits: EditLimits::default(),
        }
    }

    /// Set the positive host-owned file bound.
    ///
    /// # Errors
    /// Returns invalid input for a zero or unrepresentable bound.
    pub fn with_limits(mut self, limits: EditLimits) -> Result<Self, Error> {
        mutation::validate_limit(limits.max_file_bytes)?;
        self.limits = limits;
        Ok(self)
    }

    /// Definition derived from the executed input and result types.
    pub fn definition() -> Definition {
        Definition::new::<EditInput, EditResult>(
            "edit",
            "Replace exact old_text with new_text in an existing UTF-8 file. Read the file first and pass its revision as expected_revision to refuse stale edits. Whitespace and line endings must match exactly. Refuses zero or multiple occurrences unless replace_all: true; old_text must not be empty. Empty new_text removes the matched text, not the file. Preserves unmatched bytes and reports replacements, sizes and the new revision. Leaf symlinks are refused.",
        )
    }

    /// Prepare and commit an exact edit, checking cancellation before commit.
    ///
    /// Dropping the future requests cancellation; a commit already started
    /// may still finish. Calls sharing a workspace serialize their mutations.
    ///
    /// # Errors
    /// Returns invalid input for empty/missing/ambiguous matches, root/symlink
    /// refusal, unsupported text, size limits, I/O or pre-commit cancellation.
    pub async fn edit(
        &self,
        input: EditInput,
        cancel: &CancellationToken,
    ) -> Result<Output<EditResult>, Error> {
        let maximum = self.limits.max_file_bytes;
        self.workspace
            .run(cancel, move |root, cancel| {
                if input.old_text.is_empty() {
                    return Err(Error::InvalidInput("old_text must not be empty".into()));
                }
                mutation::bounded_text(&input.old_text, maximum)?;
                mutation::bounded_text(&input.new_text, maximum)?;
                let path = mutation::destination(root, &input.path)?;
                let relative = root.relative(&path)?;
                let before = mutation::snapshot(&path, maximum)?.ok_or_else(|| {
                    mutation::io(
                        "open the edit file",
                        std::io::Error::from(std::io::ErrorKind::NotFound),
                    )
                })?;
                revision::check(input.expected_revision.as_deref(), Some(&before.bytes))?;
                let source = mutation::validate_text(&before.bytes)?;
                let count = source.matches(&input.old_text).count();
                if count == 0 {
                    return Err(Error::InvalidInput(
                        "old_text was not found; read the file again".into(),
                    ));
                }
                if count > 1 && !input.replace_all {
                    return Err(Error::InvalidInput(format!(
                        "old_text matches {count} times; use a unique passage or replace_all: true"
                    )));
                }
                let size = source
                    .len()
                    .checked_sub(input.old_text.len().saturating_mul(count))
                    .and_then(|size| {
                        input
                            .new_text
                            .len()
                            .checked_mul(count)
                            .and_then(|added| size.checked_add(added))
                    })
                    .filter(|size| *size <= maximum)
                    .ok_or_else(|| {
                        Error::LimitExceeded("edited file exceeds max_file_bytes".into())
                    })?;
                mutation::check_cancel(cancel)?;
                let changed = input.old_text != input.new_text;
                let revision = if changed {
                    let after = source.replace(&input.old_text, &input.new_text);
                    let revision = revision::of(after.as_bytes());
                    mutation::commit(&path, Some(&before), after.as_bytes(), maximum, cancel)?;
                    revision
                } else {
                    revision::of(&before.bytes)
                };
                let data = EditResult {
                    path: relative,
                    replacements: count,
                    changed,
                    bytes_before: before.bytes.len(),
                    bytes_after: size,
                    revision,
                };
                let text = format!(
                    "edit {:?}: replacements={}, changed={}, bytes_before={}, bytes_after={}\n",
                    data.path, data.replacements, data.changed, data.bytes_before, data.bytes_after
                );
                Ok(Output::new(data, text))
            })
            .await
    }
}
