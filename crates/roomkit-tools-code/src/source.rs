use std::path::{Path, PathBuf};

use roomkit_tools_core::Error;
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;

use crate::CodeLanguage;

pub(crate) async fn root(path: &Path) -> Result<PathBuf, Error> {
    let path = fs::canonicalize(path).await.map_err(|source| Error::Io {
        operation: "resolve the workspace root",
        source,
    })?;
    if !fs::metadata(&path)
        .await
        .map_err(|source| Error::Io {
            operation: "inspect the workspace root",
            source,
        })?
        .is_dir()
    {
        return Err(Error::InvalidInput(
            "workspace root must be a directory".into(),
        ));
    }
    Ok(path)
}

pub(crate) async fn load(
    root: &Path,
    input: &str,
    language: Option<CodeLanguage>,
    max_bytes: usize,
) -> Result<(PathBuf, CodeLanguage, String), Error> {
    if input.is_empty() {
        return Err(Error::InvalidInput("path must not be empty".into()));
    }
    let path = fs::canonicalize(root.join(input))
        .await
        .map_err(|source| Error::Io {
            operation: "resolve the source path",
            source,
        })?;
    if !path.starts_with(root) {
        return Err(Error::Refused("path is outside the workspace root".into()));
    }
    let language = language.map_or_else(|| infer_language(&path), Ok)?;
    let info = fs::metadata(&path).await.map_err(|source| Error::Io {
        operation: "inspect the source file",
        source,
    })?;
    if !info.is_file() {
        return Err(Error::Unsupported(
            "code_outline accepts regular files only".into(),
        ));
    }
    let maximum = u64::try_from(max_bytes)
        .map_err(|_| Error::InvalidInput("max_file_bytes is too large".into()))?;
    if info.len() > maximum {
        return Err(Error::LimitExceeded("source exceeds max_file_bytes".into()));
    }
    let file = File::open(&path).await.map_err(|source| Error::Io {
        operation: "open the source file",
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| Error::Io {
            operation: "read the source file",
            source,
        })?;
    if bytes.len() > max_bytes {
        return Err(Error::LimitExceeded("source exceeds max_file_bytes".into()));
    }
    if bytes.contains(&0) {
        return Err(Error::Unsupported("source contains NUL bytes".into()));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| Error::Unsupported("source must be UTF-8".into()))?;
    Ok((path, language, text))
}

fn infer_language(path: &Path) -> Result<CodeLanguage, Error> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("rs") => Ok(CodeLanguage::Rust),
        Some("py" | "pyi") => Ok(CodeLanguage::Python),
        _ => Err(Error::Unsupported(
            "set language to rust or python for this extension".into(),
        )),
    }
}
