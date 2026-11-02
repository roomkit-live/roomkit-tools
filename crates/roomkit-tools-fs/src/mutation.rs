use std::fs::{self, File, Permissions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roomkit_tools_core::ToolError;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::paths::Root;

const RESOLVE_PARENT: &str = "resolve the destination parent";

/// Host-owned root and serialization shared by selected mutation tools.
///
/// Pass clones to `WriteTool::from_workspace` and `EditTool::from_workspace`
/// to serialize their complete read/modify/commit operations. Separately
/// constructed workspaces and external editors do not share this lock.
/// Write can create missing parents; other mutations require existing parents.
/// Leaf symlinks are refused.
///
/// Commits stage a synced temporary file and rename it over the destination.
/// A replaced file keeps its permission bits only: owner, group, extended
/// attributes and ACLs are not carried over, hard links to the old file keep
/// the old content, and the parent directory is not synced after the rename.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: Root,
    gate: Arc<Mutex<()>>,
}

impl Workspace {
    /// Construct a shared mutation scope beneath an existing directory.
    ///
    /// # Errors
    /// Returns invalid input for a non-directory or I/O for an unresolved root.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: Root::new(root).await?,
            gate: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) async fn run<T, F>(
        &self,
        cancel: &CancellationToken,
        work: F,
    ) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&Root, &CancellationToken) -> Result<T, ToolError> + Send + 'static,
    {
        let cancel = cancel.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let gate = tokio::select! { biased;
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            gate = Arc::clone(&self.gate).lock_owned() => gate,
        };
        let root = self.root.clone();
        // The job owns the lock even if its caller is dropped. Cancellation is
        // checked before commit; once committed, return success to a waiting
        // caller rather than racing the result against a cancelled token.
        tokio::task::spawn_blocking(move || {
            let _gate = gate;
            check_cancel(&cancel)?;
            work(&root, &cancel)
        })
        .await
        .map_err(|source| ToolError::Io {
            operation: "finish the mutation task",
            source: std::io::Error::other(source),
        })?
    }
}

pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn io(operation: &'static str, source: std::io::Error) -> ToolError {
    ToolError::Io { operation, source }
}

pub(crate) fn validate_limit(maximum: usize) -> Result<(), ToolError> {
    if maximum == 0 || u64::try_from(maximum).map_or(true, |n| n == u64::MAX) {
        return Err(ToolError::InvalidInput(
            "max_file_bytes must be positive and representable".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_text(bytes: &[u8]) -> Result<&str, ToolError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ToolError::UnsupportedResource("mutations require UTF-8 text".into()))?;
    if text.contains('\0') {
        return Err(ToolError::UnsupportedResource(
            "mutations do not accept NUL bytes".into(),
        ));
    }
    Ok(text)
}

pub(crate) fn bounded_text(text: &str, maximum: usize) -> Result<(), ToolError> {
    if text.len() > maximum {
        return Err(ToolError::LimitExceeded(
            "text exceeds max_file_bytes".into(),
        ));
    }
    validate_text(text.as_bytes())?;
    Ok(())
}

/// Resolve the existing parent, retaining the final component so a symlink
/// at the destination cannot turn into a permitted ordinary file unnoticed.
pub(crate) fn destination(root: &Root, input: &str) -> Result<PathBuf, ToolError> {
    if matches!(
        input.rsplit(std::path::MAIN_SEPARATOR).next(),
        None | Some("" | "." | "..")
    ) {
        return Err(ToolError::InvalidInput("path must name a file".into()));
    }
    let joined = root.join(input)?;
    let name = joined
        .file_name()
        .ok_or_else(|| ToolError::InvalidInput("path must name a file".into()))?;
    let parent = joined
        .parent()
        .ok_or_else(|| ToolError::InvalidInput("path must have a parent".into()))?;
    let parent = root.canonical(parent, RESOLVE_PARENT)?;
    if !parent.is_dir() {
        return Err(ToolError::UnsupportedResource(
            "destination parent must be a directory".into(),
        ));
    }
    Ok(parent.join(name))
}

/// Plan missing directories without creating anything before input/revision
/// validation. Existing-parent paths retain destination's canonical semantics.
pub(crate) fn write_destination(
    root: &Root,
    input: &str,
) -> Result<(PathBuf, Vec<PathBuf>), ToolError> {
    match destination(root, input) {
        Ok(path) => return Ok((path, Vec::new())),
        Err(ToolError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let joined = root.join(input)?;
    if joined
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(ToolError::AccessRefused(
            "missing-parent paths must not contain '..'".into(),
        ));
    }
    let name = joined
        .file_name()
        .ok_or_else(|| ToolError::InvalidInput("path must name a file".into()))?;
    let mut ancestor = joined
        .parent()
        .ok_or_else(|| ToolError::InvalidInput("path must have a parent".into()))?;
    let mut names = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if names.len() == 64 {
                    return Err(ToolError::LimitExceeded(
                        "write creates at most 64 parent directories".into(),
                    ));
                }
                names.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| ToolError::InvalidInput("invalid parent path".into()))?
                        .to_owned(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| ToolError::InvalidInput("invalid parent path".into()))?;
            }
            Err(error) => return Err(io("inspect the destination parent", error)),
        }
    }
    let mut parent = root.canonical(ancestor, RESOLVE_PARENT)?;
    if !parent.is_dir() {
        return Err(ToolError::UnsupportedResource(
            "destination parent must be a directory".into(),
        ));
    }
    let mut missing = Vec::new();
    for name in names.into_iter().rev() {
        parent.push(name);
        missing.push(parent.clone());
    }
    Ok((parent.join(name), missing))
}

/// Create planned parents under the shared workspace lock, rechecking each
/// resolved ancestor. External filesystem races remain outside this lock.
pub(crate) fn create_parents(
    root: &Root,
    paths: &[PathBuf],
    cancel: &CancellationToken,
) -> Result<Vec<String>, ToolError> {
    let mut created = Vec::new();
    for path in paths {
        check_cancel(cancel)?;
        let parent = path
            .parent()
            .ok_or_else(|| ToolError::InvalidInput("invalid parent path".into()))?;
        root.canonical(parent, RESOLVE_PARENT)?;
        match fs::create_dir(path) {
            Ok(()) => created.push(root.relative(path)?),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let resolved = fs::canonicalize(path).map_err(|e| io(RESOLVE_PARENT, e))?;
                if !resolved.starts_with(root.path()) || !resolved.is_dir() {
                    return Err(ToolError::AccessRefused(
                        "destination parent changed during creation".into(),
                    ));
                }
            }
            Err(error) => return Err(io("create a destination parent directory", error)),
        }
    }
    Ok(created)
}

pub(crate) struct Snapshot {
    pub(crate) bytes: Vec<u8>,
    permissions: Permissions,
}

pub(crate) fn inspect(path: &Path) -> Result<Option<fs::Metadata>, ToolError> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io("inspect the destination", error)),
    };
    if info.file_type().is_symlink() {
        return Err(ToolError::AccessRefused(
            "mutation of a symbolic link is not supported".into(),
        ));
    }
    if !info.is_file() {
        return Err(ToolError::UnsupportedResource(
            "mutations accept regular files only".into(),
        ));
    }
    if info.permissions().readonly() {
        return Err(ToolError::AccessRefused("destination is read-only".into()));
    }
    Ok(Some(info))
}

pub(crate) fn snapshot(path: &Path, maximum: usize) -> Result<Option<Snapshot>, ToolError> {
    let Some(info) = inspect(path)? else {
        return Ok(None);
    };
    let maximum = u64::try_from(maximum)
        .map_err(|_| ToolError::InvalidInput("file bound is too large".into()))?;
    if info.len() > maximum {
        return Err(ToolError::LimitExceeded(
            "existing file exceeds max_file_bytes".into(),
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|source| io("open the destination", source))?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| io("read the destination", source))?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > maximum) {
        return Err(ToolError::LimitExceeded(
            "existing file exceeds max_file_bytes".into(),
        ));
    }
    validate_text(&bytes)?;
    Ok(Some(Snapshot {
        bytes,
        permissions: info.permissions(),
    }))
}

pub(crate) fn commit(
    path: &Path,
    before: Option<&Snapshot>,
    after: &[u8],
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    commit_with_permissions(
        path,
        before,
        after,
        before.map(|s| s.permissions.clone()),
        maximum,
        cancel,
    )
}

pub(crate) fn create_from(
    path: &Path,
    source: &Snapshot,
    after: &[u8],
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    commit_with_permissions(
        path,
        None,
        after,
        Some(source.permissions.clone()),
        maximum,
        cancel,
    )
}

fn commit_with_permissions(
    path: &Path,
    before: Option<&Snapshot>,
    after: &[u8],
    permissions: Option<Permissions>,
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    check_cancel(cancel)?;
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::InvalidInput("path must have a parent".into()))?;
    let staged = stage(parent, after, permissions)?;
    // Best-effort detection of external changes. The check and rename are not
    // a filesystem compare-and-swap; the shared workspace lock covers our tools.
    recheck(path, before, maximum)?;
    check_cancel(cancel)?;
    if before.is_some() {
        staged
            .persist(path)
            .map_err(|error| io("replace the destination", error.error))?;
    } else {
        staged.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                ToolError::AccessRefused(
                    "destination already exists; read it before overwriting".into(),
                )
            } else {
                io("create the destination", error.error)
            }
        })?;
    }
    Ok(())
}

/// Write and sync `after` to a temporary file beside its destination.
///
/// With `permissions`, the staged file gets them (a replacement or a move
/// keeps its source's mode). Without, it is a new file: on Unix it is created
/// with mode 0o666 so that the process umask applies, as for `std::fs::write`,
/// instead of the owner-only mode of temporary files.
fn stage(
    parent: &Path,
    after: &[u8],
    permissions: Option<Permissions>,
) -> Result<tempfile::NamedTempFile, ToolError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".roomkit-write-");
    #[cfg(unix)]
    if permissions.is_none() {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(Permissions::from_mode(0o666));
    }
    let mut staged = builder
        .tempfile_in(parent)
        .map_err(|source| io("stage the file", source))?;
    staged
        .write_all(after)
        .map_err(|source| io("write the staged file", source))?;
    if let Some(permissions) = permissions {
        staged
            .as_file()
            .set_permissions(permissions)
            .map_err(|source| io("preserve file permissions", source))?;
    }
    staged
        .as_file()
        .sync_all()
        .map_err(|source| io("sync the staged file", source))?;
    Ok(staged)
}

pub(crate) fn recheck(
    path: &Path,
    before: Option<&Snapshot>,
    maximum: usize,
) -> Result<(), ToolError> {
    let current = snapshot(path, maximum)?;
    if current.as_ref().map(|s| (&s.bytes, &s.permissions))
        != before.map(|s| (&s.bytes, &s.permissions))
    {
        return Err(ToolError::AccessRefused(
            "file changed while preparing the mutation; read it again".into(),
        ));
    }
    Ok(())
}

pub(crate) fn remove(
    path: &Path,
    before: &Snapshot,
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    check_cancel(cancel)?;
    recheck(path, Some(before), maximum)?;
    check_cancel(cancel)?;
    fs::remove_file(path).map_err(|error| io("remove the patch source", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_change_is_preserved_and_staging_is_cleaned_up() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("orders");
        fs::write(&path, "original").unwrap();
        let before = snapshot(&path, 100).unwrap().unwrap();
        fs::write(&path, "external edit").unwrap();
        assert!(matches!(
            commit(
                &path,
                Some(&before),
                b"stale edit",
                100,
                &CancellationToken::new()
            ),
            Err(ToolError::AccessRefused(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn dropped_call_cancels_before_commit_and_keeps_the_lock_until_worker_exits() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("orders"), "original").unwrap();
        let workspace = Workspace::new(root.path()).await.unwrap();
        let worker = workspace.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (resume, paused) = std::sync::mpsc::channel();
        let call = tokio::spawn(async move {
            worker
                .run(&CancellationToken::new(), move |root, cancel| {
                    let path = destination(root, "orders")?;
                    let before = snapshot(&path, 100)?;
                    started.send(()).unwrap();
                    paused
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    commit(&path, before.as_ref(), b"changed", 100, cancel)
                })
                .await
        });
        ready.await.unwrap();
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        assert!(workspace.gate.try_lock().is_err());
        resume.send(()).unwrap();
        let _finished = workspace.gate.lock().await;
        assert_eq!(
            fs::read_to_string(root.path().join("orders")).unwrap(),
            "original"
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn cancellation_after_commit_does_not_report_an_unchanged_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(root.path()).await.unwrap();
        let result = workspace
            .run(&CancellationToken::new(), |root, cancel| {
                let path = destination(root, "orders")?;
                commit(&path, None, b"committed", 100, cancel)?;
                cancel.cancel();
                Ok("committed")
            })
            .await
            .unwrap();
        assert_eq!(result, "committed");
        assert_eq!(
            fs::read_to_string(root.path().join("orders")).unwrap(),
            "committed"
        );
    }
}
