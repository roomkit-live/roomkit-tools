use std::path::Path;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::paths::Root;
use crate::{Workspace, line_endings, mutation, revision};

/// An exact replacement in a text file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    /// Existing file in the workspace.
    pub path: String,
    /// Exact nonempty text, whitespace included. In an all-CRLF file, LF here and in
    /// `new_text` stands for CRLF.
    pub old_text: String,
    /// Replacement; empty removes the match.
    pub new_text: String,
    /// Replace every non-overlapping occurrence.
    #[serde(default)]
    pub replace_all: bool,
    /// Revision from read/write/edit; refuses a changed file.
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
    workspace: Workspace,
    limits: EditLimits,
}

impl EditTool {
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
            limits: EditLimits::default(),
        }
    }

    /// Set the positive host-owned file bound.
    ///
    /// # Errors
    /// Returns invalid input for a zero or unrepresentable bound.
    pub fn with_limits(mut self, limits: EditLimits) -> Result<Self, ToolError> {
        mutation::validate_limit(limits.max_file_bytes)?;
        self.limits = limits;
        Ok(self)
    }

    /// Definition derived from the executed input and result types.
    pub fn definition() -> Definition {
        Definition::new::<EditInput, EditResult>(
            "edit",
            "Replace exact old_text with new_text in an existing UTF-8 file. old_text must occur exactly once unless replace_all: true. Pass the revision from read as expected_revision to refuse a stale edit.",
        )
    }

    /// Prepare and commit an exact edit, checking cancellation before commit.
    ///
    /// In a file whose line endings are all CRLF, LF endings in the texts
    /// stand for CRLF, except after a line whose own text ends in `\r`
    /// (`read` shows it as `a\r`): `a\r` followed by LF reads as CRLF already
    /// written, so that line needs its exact ending, `a\r\r\n`.
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
    ) -> Result<Output<EditResult>, ToolError> {
        let maximum = self.limits.max_file_bytes;
        self.workspace
            .run(cancel, move |root, cancel| {
                apply(root, &input, maximum, cancel)
            })
            .await
    }
}

/// Check, match and commit one edit under the workspace lock; blocking.
fn apply(
    root: &Root,
    input: &EditInput,
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Output<EditResult>, ToolError> {
    check_arguments(input, maximum)?;
    let path = mutation::destination(root, &input.path)?;
    let relative = root.relative(&path)?;
    let before = existing(root, &input.path, &path, maximum)?;
    revision::check(input.expected_revision.as_deref(), Some(&before.bytes))?;
    let source = mutation::validate_text(&before.bytes)?;
    let crlf = line_endings::uniformly_crlf(source);
    let old_text = line_endings::in_file_endings(crlf, &input.old_text);
    let new_text = line_endings::in_file_endings(crlf, &input.new_text);
    let count = occurrences(source, &old_text, input.replace_all)?;
    let size = edited_size(source.len(), old_text.len(), new_text.len(), count, maximum)?;
    mutation::check_cancel(cancel)?;
    let changed = old_text != new_text;
    let revision = if changed {
        let after = source.replace(old_text.as_ref(), &new_text);
        let revision = revision::of(after.as_bytes());
        mutation::commit(&path, Some(&before), after.as_bytes(), maximum, cancel)?;
        revision
    } else {
        revision::of(&before.bytes)
    };
    Ok(render(EditResult {
        path: relative,
        replacements: count,
        changed,
        bytes_before: before.bytes.len(),
        bytes_after: size,
        revision,
    }))
}

/// Refuse an empty `old_text` and texts that exceed the file bound.
fn check_arguments(input: &EditInput, maximum: usize) -> Result<(), ToolError> {
    if input.old_text.is_empty() {
        return Err(ToolError::InvalidInput("old_text must not be empty".into()));
    }
    mutation::bounded_text(&input.old_text, maximum)?;
    mutation::bounded_text(&input.new_text, maximum)
}

/// The file to edit at `path`, which must exist; a missing one is named as
/// the call wrote it, `written`.
fn existing(
    root: &Root,
    written: &str,
    path: &Path,
    maximum: usize,
) -> Result<mutation::Snapshot, ToolError> {
    match mutation::snapshot(path, maximum)? {
        Some(snapshot) => Ok(snapshot),
        None => Err(root.missing(&root.join(written)?)),
    }
}

/// Occurrences of `old_text`: at least one, and exactly one unless
/// `replace_all`.
fn occurrences(source: &str, old_text: &str, replace_all: bool) -> Result<usize, ToolError> {
    let count = source.matches(old_text).count();
    if count == 0 {
        return Err(ToolError::InvalidInput(
            "old_text was not found; read the file again".into(),
        ));
    }
    if count > 1 && !replace_all {
        return Err(ToolError::InvalidInput(format!(
            "old_text matches {count} times; use a unique passage or replace_all: true"
        )));
    }
    Ok(count)
}

/// Size of the edited file, refused above the file bound.
fn edited_size(
    source: usize,
    old_text: usize,
    new_text: usize,
    count: usize,
    maximum: usize,
) -> Result<usize, ToolError> {
    source
        .checked_sub(old_text.saturating_mul(count))
        .and_then(|size| {
            new_text
                .checked_mul(count)
                .and_then(|added| size.checked_add(added))
        })
        .filter(|size| *size <= maximum)
        .ok_or_else(|| ToolError::LimitExceeded("edited file exceeds max_file_bytes".into()))
}

fn render(data: EditResult) -> Output<EditResult> {
    let text = format!(
        "edit {:?}: replacements={}, changed={}, bytes_before={}, bytes_after={}, revision={}\n",
        data.path,
        data.replacements,
        data.changed,
        data.bytes_before,
        data.bytes_after,
        data.revision
    );
    Output::new(data, text)
}
