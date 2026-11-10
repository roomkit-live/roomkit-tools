//! The ignore rules a recursive scan applies: the `.gitignore` and `.ignore`
//! files of each directory from the workspace root down, and `.git` itself.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use roomkit_tools_core::ToolError;
use tokio::fs;
use tokio::io::AsyncReadExt;

use crate::paths::{PathEntry, PathKind, Root};
use crate::worker;

/// The ignore files of a directory, in decreasing precedence, as ripgrep
/// consults them: the `.ignore` rules of every level first, the deepest
/// one deciding, then the `.gitignore` rules the same way.
const FILES: [&str; 2] = [".ignore", ".gitignore"];
/// Largest ignore file read, in bytes.
const MAX_FILE_BYTES: usize = 256 * 1024;
/// Most ignore-file bytes one scan reads across its directories, so that a
/// workspace the model writes to cannot make a scan read or compile more.
const MAX_SCAN_BYTES: usize = 2 * 1024 * 1024;

/// The rules in force in one directory: its own ignore files' and those of
/// every directory above it, up to the workspace root.
#[derive(Clone, Debug)]
pub(crate) struct Ignores {
    root: Root,
    /// For each of [`FILES`], one matcher per directory that has that file,
    /// the root's first.
    levels: [Vec<Arc<Gitignore>>; 2],
    /// Ignore-file bytes the scan may still read, shared by its directories.
    budget: Arc<Mutex<usize>>,
}

impl Ignores {
    /// The rules a scan of `directory` starts from, `None` when it includes
    /// what they would leave out.
    pub(crate) async fn for_scan(
        root: &Root,
        directory: &Path,
        include_ignored: bool,
    ) -> Result<Option<Self>, ToolError> {
        if include_ignored {
            return Ok(None);
        }
        Self::above(root, directory).await.map(Some)
    }

    /// The rules that `directory`, a canonical directory in the workspace,
    /// inherits: those of the root and of each directory down to its parent.
    async fn above(root: &Root, directory: &Path) -> Result<Self, ToolError> {
        let mut rules = Self {
            root: root.clone(),
            levels: Default::default(),
            budget: Arc::new(Mutex::new(MAX_SCAN_BYTES)),
        };
        let mut above: Vec<&Path> = directory
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor.starts_with(root.path()))
            .collect();
        above.reverse();
        for ancestor in above {
            rules = rules.enter(ancestor).await?;
        }
        Ok(rules)
    }

    /// The rules in `directory`: `inherited` with its own ignore files' rules,
    /// or `None` for a scan without rules.
    pub(crate) async fn within(
        inherited: Option<Self>,
        directory: &Path,
    ) -> Result<Option<Self>, ToolError> {
        match inherited {
            Some(rules) => rules.enter(directory).await.map(Some),
            None => Ok(None),
        }
    }

    /// These rules, with those of `directory`'s own ignore files, each
    /// compiled on a blocking worker.
    async fn enter(&self, directory: &Path) -> Result<Self, ToolError> {
        let mut rules = self.clone();
        for (levels, name) in rules.levels.iter_mut().zip(FILES) {
            let Some(text) = self.read(&directory.join(name)).await? else {
                continue;
            };
            let directory = directory.to_path_buf();
            let compiled = move || compile(directory, &text);
            levels.push(Arc::new(
                worker::blocking("compile ignore rules", compiled).await?,
            ));
        }
        Ok(rules)
    }

    /// Whether the scan leaves `entry` out: `.git`, or an entry the rule
    /// that decides for it ignores.
    pub(crate) fn ignores(&self, entry: &PathEntry) -> bool {
        if entry.name == ".git" {
            return true;
        }
        let path = self.root.path().join(&entry.path);
        let directory = entry.kind == PathKind::Directory;
        for level in self.levels.iter().flat_map(|levels| levels.iter().rev()) {
            match level.matched(&path, directory) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }

    /// An ignore file's text, charged to the scan's budget, or `None` when
    /// there is no regular file there. The read itself is bounded, whatever
    /// the file has become since it was inspected. A file the scan cannot
    /// read is named, with the way to scan without it.
    async fn read(&self, path: &Path) -> Result<Option<String>, ToolError> {
        let unreadable = |_| {
            let shown = self.root.relative(path).unwrap_or_default();
            ToolError::UnsupportedResource(format!(
                "cannot read the ignore file {shown}; set include_ignored to scan without ignore files"
            ))
        };
        match fs::symlink_metadata(path).await {
            Ok(info) if info.is_file() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(error)),
        }
        let file = fs::File::open(path).await.map_err(unreadable)?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(unreadable)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ToolError::LimitExceeded(
                "an ignore file exceeds 256 KiB; set include_ignored".into(),
            ));
        }
        self.charge(bytes.len())?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Take `bytes` from the scan's budget, or refuse once it is spent.
    fn charge(&self, bytes: usize) -> Result<(), ToolError> {
        let mut left = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
        *left = left.checked_sub(bytes).ok_or_else(|| {
            ToolError::LimitExceeded(
                "ignore files exceed 2 MiB in one scan; set include_ignored".into(),
            )
        })?;
        Ok(())
    }
}

/// The matcher of one ignore file of `directory`; blocking.
fn compile(directory: PathBuf, text: &str) -> Result<Gitignore, ToolError> {
    let mut builder = GitignoreBuilder::new(directory);
    for line in text.lines() {
        // Git skips a pattern it cannot parse, and so does the scan.
        let _skipped = builder.add_line(None, line);
    }
    builder.build().map_err(|_| {
        ToolError::UnsupportedResource(
            "an ignore file's rules cannot be compiled; set include_ignored".into(),
        )
    })
}
