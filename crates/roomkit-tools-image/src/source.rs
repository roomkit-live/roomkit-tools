use std::path::{Path, PathBuf};

use roomkit_tools_core::{Error, WorkspaceRoot};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;

pub(crate) async fn root(given: &Path) -> Result<WorkspaceRoot, Error> {
    let path = fs::canonicalize(given).await.map_err(|source| Error::Io {
        operation: "resolve the image workspace root",
        source,
    })?;
    if !fs::metadata(&path)
        .await
        .map_err(|source| Error::Io {
            operation: "inspect the image workspace root",
            source,
        })?
        .is_dir()
    {
        return Err(Error::InvalidInput(
            "workspace root must be a directory".into(),
        ));
    }
    let given = std::path::absolute(given).unwrap_or_else(|_| path.clone());
    Ok(WorkspaceRoot::new(path, &given))
}

pub(crate) async fn load(
    root: &WorkspaceRoot,
    input: &str,
    maximum: usize,
) -> Result<(PathBuf, Vec<u8>), Error> {
    let path = resolve(root, input).await?;
    let metadata = fs::metadata(&path).await.map_err(|source| Error::Io {
        operation: "inspect the image file",
        source,
    })?;
    if !metadata.is_file() {
        return Err(Error::Unsupported(
            "image input must be a regular file".into(),
        ));
    }
    let max = u64::try_from(maximum)
        .map_err(|_| Error::InvalidInput("max_file_bytes is too large".into()))?;
    if metadata.len() > max {
        return Err(Error::LimitExceeded("image exceeds max_file_bytes".into()));
    }
    let file = File::open(&path).await.map_err(|source| Error::Io {
        operation: "open the image file",
        source,
    })?;
    // Inspect the opened handle too. Workspace containment assumes no hostile
    // concurrent path replacement, as for the other root-configured tools.
    if !file
        .metadata()
        .await
        .map_err(|source| Error::Io {
            operation: "inspect the opened image file",
            source,
        })?
        .is_file()
    {
        return Err(Error::Unsupported(
            "image input must be a regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| Error::Io {
            operation: "read the image file",
            source,
        })?;
    if bytes.len() > maximum {
        return Err(Error::LimitExceeded("image exceeds max_file_bytes".into()));
    }
    Ok((path, bytes))
}

/// Join and canonicalize `input` on a blocking worker, refusing it outside
/// the root whether or not it exists; see [`WorkspaceRoot::resolve`].
async fn resolve(root: &WorkspaceRoot, input: &str) -> Result<PathBuf, Error> {
    const OPERATION: &str = "resolve the image path";
    let path = root.join(input)?;
    let root = root.clone();
    tokio::task::spawn_blocking(move || root.resolve(&path, OPERATION))
        .await
        .map_err(|source| Error::Io {
            operation: OPERATION,
            source: std::io::Error::other(source),
        })?
}
