//! Lexical workspace paths shared by root-configured tools; no I/O happens here.

use std::path::{Component, Path, PathBuf};

use crate::Error;

/// A host-selected workspace root and the path rules tools apply beneath it.
///
/// Tools canonicalize the root themselves, then use this value to refuse
/// model-supplied paths that lexically leave the workspace before any
/// filesystem call can reveal whether they exist, and to present resolved
/// paths relative to the root. It performs no I/O and resolves no symbolic
/// links: callers still canonicalize an accepted path and check the result
/// with [`WorkspaceRoot::relative`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRoot {
    canonical: PathBuf,
    alias: Option<PathBuf>,
}

impl WorkspaceRoot {
    /// Pair the canonical root with the absolute spelling the host supplied.
    ///
    /// When the host's spelling differs (for example through a symbolic link
    /// in a parent directory), absolute inputs written with it stay accepted
    /// by the lexical check; canonical containment still decides access.
    pub fn new(canonical: PathBuf, given: &Path) -> Self {
        let given = normalize(given);
        let alias = (given.is_absolute() && given != canonical).then_some(given);
        Self { canonical, alias }
    }

    /// The canonical root directory.
    pub fn path(&self) -> &Path {
        &self.canonical
    }

    /// Join a model-supplied path to the root, refusing lexical escapes.
    ///
    /// `.` and `..` are resolved without consulting the filesystem. The
    /// returned path is the unnormalized join, so a caller's canonicalization
    /// keeps the filesystem's meaning of `..` after a symbolic link.
    ///
    /// # Errors
    /// Returns invalid input for an empty path, and an access refusal for an
    /// absolute path outside the root or a relative path whose `..`
    /// components leave it.
    pub fn join(&self, input: &str) -> Result<PathBuf, Error> {
        if input.is_empty() {
            return Err(Error::InvalidInput("path must not be empty".into()));
        }
        let joined = self.canonical.join(input);
        let lexical = normalize(&joined);
        if lexical.starts_with(&self.canonical)
            || self
                .alias
                .as_ref()
                .is_some_and(|alias| lexical.starts_with(alias))
        {
            Ok(joined)
        } else {
            Err(outside())
        }
    }

    /// Present a resolved path relative to the canonical root, with `/`
    /// separators and `.` for the root itself.
    ///
    /// # Errors
    /// Returns an access refusal outside the root, or unsupported for a path
    /// that is not valid UTF-8.
    pub fn relative(&self, resolved: &Path) -> Result<String, Error> {
        let relative = resolved
            .strip_prefix(&self.canonical)
            .map_err(|_| outside())?;
        let relative = relative
            .to_str()
            .ok_or_else(|| Error::Unsupported("path is not UTF-8".into()))?;
        Ok(if relative.is_empty() {
            ".".into()
        } else {
            relative.replace(std::path::MAIN_SEPARATOR, "/")
        })
    }
}

fn outside() -> Error {
    Error::Refused("path is outside the workspace root".into())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normal.push(component);
            }
        }
    }
    normal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn lexical_escapes_are_refused_and_internal_paths_are_joined_unchanged() {
        let root = WorkspaceRoot::new("/work/root".into(), Path::new("/alias/root/./"));
        for inside in [
            "a.txt",
            "./a/../b",
            "/work/root/a",
            "/work/root",
            "/alias/root/a",
            "sub/../../root/x",
        ] {
            let joined = root.join(inside).unwrap();
            assert_eq!(joined, Path::new("/work/root").join(inside), "{inside}");
        }
        for outside in [
            "..",
            "../root2",
            "a/../../x",
            "/work/root2",
            "/etc/passwd",
            "/alias",
            "/alias/root/../x",
        ] {
            assert!(
                matches!(root.join(outside), Err(Error::Refused(_))),
                "{outside}"
            );
        }
        assert!(matches!(root.join(""), Err(Error::InvalidInput(_))));
    }

    #[cfg(unix)]
    #[test]
    fn resolved_paths_are_presented_relative_to_the_canonical_root() {
        let root = WorkspaceRoot::new("/work/root".into(), Path::new("/work/root"));
        assert_eq!(root.relative(Path::new("/work/root")).unwrap(), ".");
        assert_eq!(root.relative(Path::new("/work/root/a/b")).unwrap(), "a/b");
        assert!(matches!(
            root.relative(Path::new("/work/other")),
            Err(Error::Refused(_))
        ));
    }
}
