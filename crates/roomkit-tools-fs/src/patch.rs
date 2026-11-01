use std::path::Path;

use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::patch_prepare::{Job, prepare};
use crate::{DiffLimits, DiffResult, MutationWorkspace, mutation};

/// A version-1 structured patch operation. Text expectations match whole files exactly.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchChange {
    /// Create an absent file. Its parent must already exist.
    Add {
        /// New workspace file.
        path: String,
        /// Complete new UTF-8 text.
        content: String,
    },
    /// Replace an existing file only if its complete text matches.
    Update {
        /// Existing workspace file.
        path: String,
        /// Exact expected contents, including line endings and BOM (LF stands
        /// for CRLF when the file uses CRLF throughout).
        old_content: String,
        /// Complete replacement contents.
        new_content: String,
    },
    /// Delete an existing file only if its complete text matches.
    Delete {
        /// Existing workspace file.
        path: String,
        /// Exact expected contents, including line endings and BOM (LF stands
        /// for CRLF when the file uses CRLF throughout).
        old_content: String,
    },
    /// Create an absent destination, then remove the matching source.
    Move {
        /// Existing workspace source.
        from: String,
        /// Absent workspace destination. Its parent must exist.
        to: String,
        /// Exact expected source contents (LF stands for CRLF when the source
        /// uses CRLF throughout, as for `new_content`).
        old_content: String,
        /// Optional replacement contents; omitted or null keeps the source bytes.
        #[serde(default)]
        new_content: Option<String>,
    },
}

/// Prepare or apply a bounded batch of exact file changes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchInput {
    /// Independent changes; no source or destination may appear more than once.
    pub changes: Vec<PatchChange>,
    /// Preview only by default. Explicit false applies the same validated input.
    #[serde(default = "preview")]
    pub dry_run: bool,
}

const fn preview() -> bool {
    true
}

impl PatchInput {
    /// Prepare a read-only preview of these changes.
    pub fn new(changes: Vec<PatchChange>) -> Self {
        Self {
            changes,
            dry_run: true,
        }
    }
}

/// Host-owned bounds for a complete patch and its previews.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct PatchLimits {
    /// Maximum operations per call. Default: 32.
    pub max_changes: usize,
    /// Maximum combined path and text argument bytes. Default: 16 MiB.
    pub max_input_bytes: usize,
    /// Per-file and comparison limits, plus total rendered preview limit.
    pub diff: DiffLimits,
}

impl Default for PatchLimits {
    fn default() -> Self {
        Self {
            max_changes: 32,
            max_input_bytes: 16 * 1024 * 1024,
            diff: DiffLimits::default(),
        }
    }
}

/// Intended operation, separate from the individual filesystem effects.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchOperation {
    /// Create a new file.
    Add,
    /// Change an existing file.
    Update,
    /// Remove an existing file.
    Delete,
    /// Create a destination and then remove the source.
    Move,
}

/// A prepared change; a preview is not evidence that it has been applied.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PatchPreview {
    /// One-based index in the requested changes.
    pub index: usize,
    /// Requested operation.
    pub operation: PatchOperation,
    /// Workspace-relative source, or new path for additions.
    pub path: String,
    /// Move destination, absent for other operations.
    pub to: Option<String>,
    /// Whether this operation would change bytes or file existence.
    pub changed: bool,
    /// Exact comparison of the proposed contents.
    pub diff: DiffResult,
    /// Unified-style content preview, not a serialized patch input.
    pub text: String,
}

/// A completed filesystem effect, including effects before a partial failure.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PatchApplied {
    /// One-based operation index. A move can produce two effects.
    pub index: usize,
    /// Actual effect: add, update or delete (never move).
    pub operation: PatchOperation,
    /// Affected workspace-relative file.
    pub path: String,
}

/// A failure after at least one completed filesystem effect.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PatchFailure {
    /// One-based index at which applying stopped.
    pub index: usize,
    /// True for explicit cancellation between effects.
    pub cancelled: bool,
    /// Model-safe error, excluding underlying I/O diagnostic paths.
    pub message: String,
}

/// Preview or application result; always inspect completed and failure.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PatchResult {
    /// True means no mutation was attempted.
    pub dry_run: bool,
    /// Whether the requested preview or full application completed.
    pub completed: bool,
    /// Prepared changes, including those not applied after a failure.
    pub previews: Vec<PatchPreview>,
    /// Effects already committed. Empty for previews and unchanged updates.
    pub applied: Vec<PatchApplied>,
    /// Failure after partial application; no rollback is promised.
    pub failure: Option<PatchFailure>,
}

/// Independently selectable structured multi-file mutations.
#[derive(Clone, Debug)]
pub struct PatchTool {
    workspace: MutationWorkspace,
    limits: PatchLimits,
}

impl PatchTool {
    /// Select a root with a new mutation scope.
    ///
    /// # Errors
    /// Returns invalid-root or I/O failures.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self::from_workspace(MutationWorkspace::new(root).await?))
    }

    /// Share serialization with write and edit.
    pub fn from_workspace(workspace: MutationWorkspace) -> Self {
        Self {
            workspace,
            limits: PatchLimits::default(),
        }
    }

    /// Configure resource bounds, including complete preview size.
    ///
    /// # Errors
    /// Rejects zero or unrepresentable bounds.
    pub fn with_limits(mut self, limits: PatchLimits) -> Result<Self, Error> {
        limits.diff.validate()?;
        if limits.max_changes == 0 || limits.max_input_bytes == 0 {
            return Err(Error::InvalidInput("patch limits must be positive".into()));
        }
        self.limits = limits;
        Ok(self)
    }

    /// Derived schema for structured patch version 1 (not a textual Git/Codex patch).
    pub fn definition() -> Definition {
        Definition::new::<PatchInput, PatchResult>(
            "patch",
            "Preview or apply a JSON batch of add/update/delete/move changes. dry_run defaults to true; use false to apply. Update/delete/move require exact complete old_content; in a file whose line endings are all CRLF, LF text stands for CRLF in old_content and new_content. Paths must be distinct, parents must exist, and add/move destinations must be absent. All changes are prepared before writing. No multi-file rollback: inspect completed, applied and failure before reporting success. This tool takes structured changes, not Git or Codex patch text.",
        )
    }

    /// Prepare all changes under the shared lock, then optionally apply in order.
    ///
    /// # Errors
    /// Preparation and failures before any effect return typed errors. A failure
    /// after committed effects returns a partial result. Dropped calls request
    /// cancellation; an in-progress single-file commit may still finish.
    pub async fn patch(
        &self,
        input: PatchInput,
        cancel: &CancellationToken,
    ) -> Result<Output<PatchResult>, Error> {
        let limits = self.limits;
        self.workspace
            .run(cancel, move |root, cancel| {
                let jobs = prepare(root, &input.changes, limits, cancel)?;
                apply(&jobs, input.dry_run, limits.diff.max_file_bytes, cancel)
            })
            .await
    }
}

fn apply(
    jobs: &[Job],
    dry_run: bool,
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Output<PatchResult>, Error> {
    let mut applied = Vec::new();
    let mut failure = None;
    if dry_run {
        mutation::check_cancel(cancel)?;
    } else {
        for job in jobs {
            if let Err(error) = apply_one(job, maximum, cancel, &mut applied) {
                if applied.is_empty() {
                    return Err(error);
                }
                failure = Some(PatchFailure {
                    index: job.preview.index,
                    cancelled: matches!(error, Error::Cancelled),
                    message: error.to_string(),
                });
                break;
            }
        }
    }
    let data = PatchResult {
        dry_run,
        completed: failure.is_none(),
        previews: jobs.iter().map(|job| job.preview.clone()).collect(),
        applied,
        failure,
    };
    let text = format!(
        "patch: dry_run={}, completed={}, applied={:?}, failure={:?}\nPrepared previews (not proof of application):\n{}",
        data.dry_run,
        data.completed,
        data.applied,
        data.failure,
        data.previews
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>()
    );
    Ok(Output::new(data, text))
}

fn apply_one(
    job: &Job,
    maximum: usize,
    cancel: &CancellationToken,
    applied: &mut Vec<PatchApplied>,
) -> Result<(), Error> {
    mutation::check_cancel(cancel)?;
    let record = |operation, path| PatchApplied {
        index: job.preview.index,
        operation,
        path,
    };
    if let Some(target) = &job.target {
        let before = job
            .before
            .as_ref()
            .ok_or_else(|| Error::InvalidInput("move requires a source".into()))?;
        mutation::recheck(&job.source, Some(before), maximum)?;
        mutation::create_from(
            target,
            before,
            job.after.as_deref().unwrap_or("").as_bytes(),
            maximum,
            cancel,
        )?;
        applied.push(record(
            PatchOperation::Add,
            job.preview.to.clone().unwrap_or_default(),
        ));
        mutation::remove(&job.source, before, maximum, cancel)?;
        applied.push(record(PatchOperation::Delete, job.preview.path.clone()));
    } else if let Some(after) = &job.after {
        if job.preview.changed {
            mutation::commit(
                &job.source,
                job.before.as_ref(),
                after.as_bytes(),
                maximum,
                cancel,
            )?;
            applied.push(record(job.preview.operation, job.preview.path.clone()));
        } else {
            mutation::recheck(&job.source, job.before.as_ref(), maximum)?;
        }
    } else {
        let before = job
            .before
            .as_ref()
            .ok_or_else(|| Error::InvalidInput("delete requires a source".into()))?;
        mutation::remove(&job.source, before, maximum, cancel)?;
        applied.push(record(PatchOperation::Delete, job.preview.path.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Root;

    #[tokio::test]
    async fn external_conflict_after_preparation_returns_actual_committed_effects() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("second"), "original").unwrap();
        let root = Root::new(folder.path()).await.unwrap();
        let cancel = CancellationToken::new();
        let jobs = prepare(
            &root,
            &[
                PatchChange::Add {
                    path: "first".into(),
                    content: "committed".into(),
                },
                PatchChange::Update {
                    path: "second".into(),
                    old_content: "original".into(),
                    new_content: "stale".into(),
                },
            ],
            PatchLimits::default(),
            &cancel,
        )
        .unwrap();
        std::fs::write(folder.path().join("second"), "external edit").unwrap();
        let result = apply(&jobs, false, 1024, &cancel).unwrap();
        assert!(!result.data.completed);
        assert_eq!(result.data.failure.as_ref().unwrap().index, 2);
        assert_eq!(result.data.applied.len(), 1);
        assert_eq!(result.data.applied[0].path, "first");
        assert_eq!(
            std::fs::read_to_string(folder.path().join("first")).unwrap(),
            "committed"
        );
        assert_eq!(
            std::fs::read_to_string(folder.path().join("second")).unwrap(),
            "external edit"
        );
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn cancellation_after_preparation_leaves_all_files_untouched() {
        let folder = tempfile::tempdir().unwrap();
        let root = Root::new(folder.path()).await.unwrap();
        let cancel = CancellationToken::new();
        let jobs = prepare(
            &root,
            &[PatchChange::Add {
                path: "new".into(),
                content: "x".into(),
            }],
            PatchLimits::default(),
            &cancel,
        )
        .unwrap();
        cancel.cancel();
        assert!(matches!(
            apply(&jobs, false, 1024, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
}
