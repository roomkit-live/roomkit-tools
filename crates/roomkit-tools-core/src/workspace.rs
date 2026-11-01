//! Workspace paths shared by root-configured tools: the lexical check made
//! before any filesystem call, then canonical containment. No I/O happens
//! here; callers supply canonicalization.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::Error;

/// A host-selected workspace root and the path rules tools apply beneath it.
///
/// Tools canonicalize the root themselves, then use this value to refuse
/// model-supplied paths that lexically leave the workspace, at any point,
/// before any filesystem call can reveal whether they exist
/// ([`WorkspaceRoot::join`]); to refuse an accepted path that canonicalizes
/// outside the root ([`WorkspaceRoot::resolve`], with the caller's
/// canonicalization); and to present resolved paths relative to the root
/// ([`WorkspaceRoot::relative`]). It performs no I/O itself.
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
    /// A spelling with a `..` component gives no alias: after a symbolic link,
    /// `..` need not lead where its lexical form says, so the lexical form
    /// could name an unrelated directory.
    pub fn new(canonical: PathBuf, given: &Path) -> Self {
        let parent = given.components().any(|c| c == Component::ParentDir);
        let given: PathBuf = given.components().collect();
        let alias = (given.is_absolute() && !parent && given != canonical).then_some(given);
        Self { canonical, alias }
    }

    /// The canonical root directory.
    pub fn path(&self) -> &Path {
        &self.canonical
    }

    /// Join a model-supplied path to the root, refusing lexical escapes.
    ///
    /// An absolute path must start with the canonical root or the host's
    /// spelling of it, and that prefix is removed. The rest, like a relative
    /// path, is walked component by component without consulting the
    /// filesystem: a `..` that would go above the root is refused, even when
    /// later components come back inside. The returned path is the canonical
    /// root joined with that rest, unnormalized, so a caller's
    /// canonicalization keeps the filesystem's meaning of `..` after a
    /// symbolic link and never looks up a directory the input names outside
    /// the root.
    ///
    /// # Errors
    /// Returns invalid input for an empty path, and an access refusal for an
    /// absolute path that does not start with the root, or a path whose `..`
    /// components go above the root at any point.
    pub fn join(&self, input: &str) -> Result<PathBuf, Error> {
        if input.is_empty() {
            return Err(Error::InvalidInput("path must not be empty".into()));
        }
        let input = Path::new(input);
        let below = if input.is_absolute() {
            self.below_root(input)?
        } else {
            input
        };
        stays_below(below)?;
        Ok(self.canonical.join(below))
    }

    /// The part of an absolute path below the canonical root or its alias.
    fn below_root<'a>(&self, input: &'a Path) -> Result<&'a Path, Error> {
        std::iter::once(&self.canonical)
            .chain(&self.alias)
            .find_map(|root| input.strip_prefix(root).ok())
            .ok_or_else(outside)
    }

    /// Canonicalize a path that [`WorkspaceRoot::join`] returned, or one of
    /// its parents, with `canonicalize`, and refuse a result outside the root.
    ///
    /// When the path cannot be canonicalized, for example because it does
    /// not exist, its deepest ancestor that can be decides: outside the root,
    /// the path is refused, so the error does not reveal whether something
    /// exists beyond a symbolic link that leaves the workspace; inside, the
    /// failure is returned as I/O with `operation`. Tools pass
    /// [`std::fs::canonicalize`], whose calls block: async callers run this on
    /// a blocking worker.
    ///
    /// # Errors
    /// Returns an access refusal outside the root, or I/O when the path
    /// cannot be canonicalized.
    pub fn resolve(
        &self,
        path: &Path,
        operation: &'static str,
        canonicalize: impl Fn(&Path) -> io::Result<PathBuf>,
    ) -> Result<PathBuf, Error> {
        match canonicalize(path) {
            Ok(resolved) => self.contain(resolved),
            Err(source) => {
                self.refuse_outside_ancestor(path, canonicalize)?;
                Err(Error::Io { operation, source })
            }
        }
    }

    fn contain(&self, resolved: PathBuf) -> Result<PathBuf, Error> {
        if resolved.starts_with(&self.canonical) {
            Ok(resolved)
        } else {
            Err(outside())
        }
    }

    /// Refuse `path` when its deepest ancestor that canonicalizes, at or
    /// below the root, resolves outside the root.
    fn refuse_outside_ancestor(
        &self,
        path: &Path,
        canonicalize: impl Fn(&Path) -> io::Result<PathBuf>,
    ) -> Result<(), Error> {
        let resolved = path
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor.starts_with(&self.canonical))
            .find_map(|ancestor| canonicalize(ancestor).ok());
        match resolved {
            Some(resolved) => self.contain(resolved).map(drop),
            None => Ok(()),
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

/// Refuse a path below the root that is not relative, or whose `..`
/// components go above its start at any point.
fn stays_below(path: &Path) -> Result<(), Error> {
    let mut depth = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => depth = depth.checked_sub(1).ok_or_else(outside)?,
            Component::Prefix(_) | Component::RootDir => return Err(outside()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn lexical_escapes_are_refused_and_internal_paths_are_joined_below_the_root() {
        let root = WorkspaceRoot::new("/work/root".into(), Path::new("/alias/root/./"));
        for (inside, joined) in [
            ("a.txt", "/work/root/a.txt"),
            ("./a/../b", "/work/root/a/../b"),
            ("/work/root/a", "/work/root/a"),
            ("/work/root/a/../b", "/work/root/a/../b"),
            ("/work/root", "/work/root"),
            ("/alias/root/a", "/work/root/a"),
        ] {
            assert_eq!(root.join(inside).unwrap(), Path::new(joined), "{inside}");
        }
        for outside in [
            "..",
            "../root2",
            "a/../../x",
            "sub/../../root/x",
            "/work/root2",
            "/etc/passwd",
            "/alias",
            "/alias/root/../x",
            "/alias/root/../root/x",
            "/work/root/../root/x",
            "/work/../work/root/x",
            "/etc/../work/root/x",
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
    fn a_host_spelling_with_dot_dot_is_not_an_alias() {
        let root = WorkspaceRoot::new("/work/root".into(), Path::new("/link/../root"));
        assert_eq!(root.alias, None);
        for outside in ["/root/a", "/link/../root/a"] {
            assert!(
                matches!(root.join(outside), Err(Error::Refused(_))),
                "{outside}"
            );
        }
        assert!(root.join("/work/root/a").is_ok());
    }

    /// A fake filesystem: `/work/root` holds `sub`, a file `sub/a.txt` and
    /// `link`, a symbolic link to `/outside`, which holds `dir`.
    fn canonicalize(path: &Path) -> io::Result<PathBuf> {
        let resolved = match path.to_str() {
            Some("/work/root") => "/work/root",
            Some("/work/root/sub") => "/work/root/sub",
            Some("/work/root/sub/a.txt") => "/work/root/sub/a.txt",
            Some("/work/root/link") => "/outside",
            Some("/work/root/link/dir") => "/outside/dir",
            Some("/work/root/link/..") => "/",
            _ => return Err(io::ErrorKind::NotFound.into()),
        };
        Ok(PathBuf::from(resolved))
    }

    #[cfg(unix)]
    #[test]
    fn a_path_is_refused_when_it_or_its_deepest_resolvable_ancestor_is_outside() {
        let root = WorkspaceRoot::new("/work/root".into(), Path::new("/work/root"));
        let resolve = |input: &str| root.resolve(&root.join(input).unwrap(), "op", canonicalize);
        assert_eq!(
            resolve("sub/a.txt").unwrap(),
            Path::new("/work/root/sub/a.txt")
        );
        for outside in [
            "link",
            "link/dir",
            "link/missing",
            "link/dir/missing/x",
            "link/../x",
        ] {
            let result = resolve(outside);
            assert!(
                matches!(result, Err(Error::Refused(_))),
                "{outside}: {result:?}"
            );
        }
        for missing in ["missing", "sub/missing/x", "sub/../missing"] {
            let result = resolve(missing);
            assert!(
                matches!(
                    result,
                    Err(Error::Io {
                        operation: "op",
                        ..
                    })
                ),
                "{missing}: {result:?}"
            );
        }
    }

    /// The ancestor walk stops at the root: with no resolvable ancestor at or
    /// below it, the failure stays I/O instead of judging `/`.
    #[cfg(unix)]
    #[test]
    fn the_ancestor_walk_does_not_go_above_the_root() {
        let root = WorkspaceRoot::new("/gone/root".into(), Path::new("/gone/root"));
        let result = root.resolve(&root.join("missing").unwrap(), "op", |path: &Path| {
            if path == Path::new("/") {
                Ok(PathBuf::from("/"))
            } else {
                Err(io::ErrorKind::NotFound.into())
            }
        });
        assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
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
