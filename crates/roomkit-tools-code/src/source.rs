use std::path::{Path, PathBuf};

use roomkit_tools_core::{ToolError, WorkspaceRoot};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;

use crate::CodeLanguage;

pub(crate) async fn root(given: &Path) -> Result<WorkspaceRoot, ToolError> {
    let path = fs::canonicalize(given)
        .await
        .map_err(|source| ToolError::Io {
            operation: "resolve the workspace root",
            source,
        })?;
    if !fs::metadata(&path)
        .await
        .map_err(|source| ToolError::Io {
            operation: "inspect the workspace root",
            source,
        })?
        .is_dir()
    {
        return Err(ToolError::InvalidInput(
            "workspace root must be a directory".into(),
        ));
    }
    let given = std::path::absolute(given).unwrap_or_else(|_| path.clone());
    Ok(WorkspaceRoot::new(path, &given))
}

pub(crate) async fn load(
    root: &WorkspaceRoot,
    input: &str,
    language: Option<CodeLanguage>,
    max_bytes: usize,
) -> Result<(PathBuf, CodeLanguage, String), ToolError> {
    let path = resolve(root, input).await?;
    let language = language.map_or_else(|| infer_language(&path), Ok)?;
    let info = fs::metadata(&path).await.map_err(|source| ToolError::Io {
        operation: "inspect the source file",
        source,
    })?;
    if !info.is_file() {
        return Err(ToolError::UnsupportedResource(
            "code_outline accepts regular files only".into(),
        ));
    }
    let maximum = u64::try_from(max_bytes)
        .map_err(|_| ToolError::InvalidInput("max_file_bytes is too large".into()))?;
    if info.len() > maximum {
        return Err(ToolError::LimitExceeded(
            "source exceeds max_file_bytes".into(),
        ));
    }
    let file = File::open(&path).await.map_err(|source| ToolError::Io {
        operation: "open the source file",
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| ToolError::Io {
            operation: "read the source file",
            source,
        })?;
    if bytes.len() > max_bytes {
        return Err(ToolError::LimitExceeded(
            "source exceeds max_file_bytes".into(),
        ));
    }
    if bytes.contains(&0) {
        return Err(ToolError::UnsupportedResource(
            "source contains NUL bytes".into(),
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| ToolError::UnsupportedResource("source must be UTF-8".into()))?;
    Ok((path, language, text))
}

/// Join and canonicalize `input` on a blocking worker, refusing it outside
/// the root whether or not it exists; see [`WorkspaceRoot::resolve`].
async fn resolve(root: &WorkspaceRoot, input: &str) -> Result<PathBuf, ToolError> {
    const OPERATION: &str = "resolve the source path";
    let path = root.join(input)?;
    let root = root.clone();
    tokio::task::spawn_blocking(move || {
        root.resolve(&path, OPERATION, |path| std::fs::canonicalize(path))
    })
    .await
    .map_err(|source| ToolError::Io {
        operation: OPERATION,
        source: std::io::Error::other(source),
    })?
}

fn infer_language(path: &Path) -> Result<CodeLanguage, ToolError> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("rs") => Ok(CodeLanguage::Rust),
        Some("py" | "pyi") => Ok(CodeLanguage::Python),
        _ => Err(ToolError::UnsupportedResource(
            "set language to rust or python for this extension".into(),
        )),
    }
}
