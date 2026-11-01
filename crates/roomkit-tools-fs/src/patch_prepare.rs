//! Patch preparation: every change is checked against its files and the
//! patch's bounds, and previewed, before anything is written.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use roomkit_tools_core::Error;
use tokio_util::sync::CancellationToken;

use crate::patch::{PatchChange, PatchLimits, PatchOperation, PatchPreview};
use crate::paths::Root;
use crate::{diff, line_endings, mutation};

/// A change ready to apply, with the source snapshot it was checked against.
pub(crate) struct Job {
    pub(crate) source: PathBuf,
    pub(crate) target: Option<PathBuf>,
    pub(crate) before: Option<mutation::Snapshot>,
    pub(crate) after: Option<String>,
    pub(crate) preview: PatchPreview,
}

type ChangeParts<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    PatchOperation,
);

fn parts(change: &PatchChange) -> ChangeParts<'_> {
    match change {
        PatchChange::Add { path, content } => (
            path,
            None,
            None,
            Some(content.as_str()),
            PatchOperation::Add,
        ),
        PatchChange::Update {
            path,
            old_content,
            new_content,
        } => (
            path,
            None,
            Some(old_content.as_str()),
            Some(new_content.as_str()),
            PatchOperation::Update,
        ),
        PatchChange::Delete { path, old_content } => (
            path,
            None,
            Some(old_content.as_str()),
            None,
            PatchOperation::Delete,
        ),
        PatchChange::Move {
            from,
            to,
            old_content,
            new_content,
        } => (
            from,
            Some(to.as_str()),
            Some(old_content.as_str()),
            Some(new_content.as_deref().unwrap_or(old_content)),
            PatchOperation::Move,
        ),
    }
}

/// Check and preview every change before anything is written; blocking.
pub(crate) fn prepare(
    root: &Root,
    changes: &[PatchChange],
    limits: PatchLimits,
    cancel: &CancellationToken,
) -> Result<Vec<Job>, Error> {
    if changes.is_empty() || changes.len() > limits.max_changes {
        return Err(Error::InvalidInput(
            "patch requires 1..max_changes operations".into(),
        ));
    }
    let mut totals = Totals::default();
    let mut jobs = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        mutation::check_cancel(cancel)?;
        jobs.push(prepare_change(
            root,
            index + 1,
            change,
            limits,
            cancel,
            &mut totals,
        )?);
    }
    Ok(jobs)
}

/// Running amounts that bound a whole patch.
#[derive(Default)]
struct Totals {
    input_bytes: usize,
    output_bytes: usize,
    touched: BTreeSet<PathBuf>,
}

impl Totals {
    fn count_input(&mut self, parts: [Option<&str>; 4], maximum: usize) -> Result<(), Error> {
        for part in parts.into_iter().flatten() {
            self.input_bytes = self
                .input_bytes
                .checked_add(part.len())
                .filter(|n| *n <= maximum)
                .ok_or_else(|| Error::LimitExceeded("patch exceeds max_input_bytes".into()))?;
        }
        Ok(())
    }

    fn touch(&mut self, source: &Path, target: Option<&Path>) -> Result<(), Error> {
        for path in std::iter::once(source).chain(target) {
            if !self.touched.insert(path.to_path_buf()) {
                return Err(Error::InvalidInput(
                    "patch paths overlap; each file may be touched once".into(),
                ));
            }
        }
        Ok(())
    }

    fn count_output(&mut self, heading: usize, text: usize, maximum: usize) -> Result<(), Error> {
        self.output_bytes = self
            .output_bytes
            .checked_add(heading)
            .and_then(|n| n.checked_add(text))
            .filter(|n| *n <= maximum)
            .ok_or_else(|| Error::LimitExceeded("patch previews exceed max_output_bytes".into()))?;
        Ok(())
    }
}

/// One change with its files resolved and its texts in the source's endings.
struct Resolved<'a> {
    index: usize,
    operation: PatchOperation,
    source: &'a Path,
    target: Option<&'a Path>,
    existed: bool,
    expected: Option<&'a str>,
    after: Option<&'a str>,
}

/// Check one change against its files and the patch's totals, and render
/// its preview; blocking.
fn prepare_change(
    root: &Root,
    index: usize,
    change: &PatchChange,
    limits: PatchLimits,
    cancel: &CancellationToken,
    totals: &mut Totals,
) -> Result<Job, Error> {
    let (path, to, expected, after, operation) = parts(change);
    totals.count_input([Some(path), to, expected, after], limits.max_input_bytes)?;
    for text in [expected, after].into_iter().flatten() {
        mutation::bounded_text(text, limits.diff.max_file_bytes)?;
    }
    let source = mutation::destination(root, path)?;
    let target = to
        .map(|path| mutation::destination(root, path))
        .transpose()?;
    totals.touch(&source, target.as_deref())?;
    let before = mutation::snapshot(&source, limits.diff.max_file_bytes)?;
    let (expected, after) =
        in_source_endings(before.as_ref(), expected, after, limits.diff.max_file_bytes)?;
    let (expected, after) = (expected.as_deref(), after.as_deref());
    check_existing(before.as_ref(), expected, target.as_deref())?;
    let resolved = Resolved {
        index,
        operation,
        source: &source,
        target: target.as_deref(),
        existed: before.is_some(),
        expected,
        after,
    };
    let preview = render_preview(root, &resolved, limits, cancel, totals)?;
    let after = after.map(str::to_owned);
    Ok(Job {
        source,
        target,
        before,
        after,
        preview,
    })
}

/// Refuse a source whose contents or existence differ from `expected`, and
/// a move destination that already exists.
fn check_existing(
    before: Option<&mutation::Snapshot>,
    expected: Option<&str>,
    target: Option<&Path>,
) -> Result<(), Error> {
    if before.map(|s| s.bytes.as_slice()) != expected.map(str::as_bytes) {
        return Err(Error::Refused(
            "patch source differs from old_content or required existence; read it again".into(),
        ));
    }
    if let Some(target) = target
        && mutation::inspect(target)?.is_some()
    {
        return Err(Error::Refused(
            "patch move destination already exists".into(),
        ));
    }
    Ok(())
}

/// Compare a change's contents and count its preview toward the patch's
/// output bound.
fn render_preview(
    root: &Root,
    change: &Resolved<'_>,
    limits: PatchLimits,
    cancel: &CancellationToken,
    totals: &mut Totals,
) -> Result<PatchPreview, Error> {
    let path = root.relative(change.source)?;
    let to = change.target.map(|path| root.relative(path)).transpose()?;
    let old_label = label(change.existed, &path);
    let new_label = label(change.after.is_some(), to.as_deref().unwrap_or(&path));
    let compared = diff::compare(
        change.expected.unwrap_or(""),
        change.after.unwrap_or(""),
        old_label,
        new_label,
        3,
        limits.diff,
        cancel,
    )?;
    let operation = change.operation;
    let heading = format!("{}: {operation:?} {path:?} -> {to:?}\n", change.index);
    totals.count_output(
        heading.len(),
        compared.text.len(),
        limits.diff.max_output_bytes,
    )?;
    Ok(PatchPreview {
        index: change.index,
        operation,
        path,
        to,
        changed: operation != PatchOperation::Update || change.expected != change.after,
        diff: compared.data,
        text: format!("{heading}{}", compared.text),
    })
}

/// A preview's name for one side of a change: the file, or `/dev/null`
/// when that side does not exist.
fn label(exists: bool, path: &str) -> &str {
    if exists { path } else { "/dev/null" }
}

type Contents<'a> = (Option<Cow<'a, str>>, Option<Cow<'a, str>>);

/// Expected and new contents in the source's line endings. `read` shows CRLF
/// lines without `\r`; when the existing source uses CRLF throughout, LF
/// endings in both texts stand for CRLF. Other sources compare exactly.
fn in_source_endings<'a>(
    before: Option<&mutation::Snapshot>,
    expected: Option<&'a str>,
    after: Option<&'a str>,
    maximum: usize,
) -> Result<Contents<'a>, Error> {
    let crlf = before
        .and_then(|snapshot| std::str::from_utf8(&snapshot.bytes).ok())
        .is_some_and(line_endings::uniformly_crlf);
    let expected = expected.map(|text| line_endings::in_file_endings(crlf, text));
    let after = after.map(|text| line_endings::in_file_endings(crlf, text));
    if let Some(text) = &after {
        mutation::bounded_text(text, maximum)?;
    }
    Ok((expected, after))
}
