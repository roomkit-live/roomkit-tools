//! The ignore rules a recursive scan applies: the `.gitignore` and `.ignore`
//! files of each directory from the repository's root down, the
//! repository's `info/exclude`, and `.git` itself. Without a repository
//! above the workspace, the workspace root is where they start.

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
/// Largest `.git` file or `commondir` file read to find a worktree's
/// repository, in bytes.
const MAX_POINTER_BYTES: u64 = 4096;

/// The rules in force in one directory: its own ignore files' and those of
/// every directory above it, up to the repository's root.
#[derive(Clone, Debug)]
pub(crate) struct Ignores {
    root: Root,
    /// For each of [`FILES`], one matcher per directory that has that file,
    /// the outermost first.
    levels: [Vec<Arc<Gitignore>>; 2],
    /// The repository's `info/exclude` rules, which decide last.
    exclude: Option<Arc<Gitignore>>,
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
    /// inherits: those of the repository above the workspace, then those of
    /// the root and of each directory down to its parent.
    async fn above(root: &Root, directory: &Path) -> Result<Self, ToolError> {
        let mut rules = Self {
            root: root.clone(),
            levels: Default::default(),
            exclude: None,
            budget: Arc::new(Mutex::new(MAX_SCAN_BYTES)),
        };
        if let Some(repository) = repository(root.path()).await {
            rules.exclude = rules.exclude_rules(&repository).await?;
            let mut outside: Vec<&Path> = root
                .path()
                .ancestors()
                .skip(1)
                .take_while(|ancestor| ancestor.starts_with(&repository))
                .collect();
            outside.reverse();
            for ancestor in outside {
                rules = rules.enter(ancestor).await?;
            }
        }
        let mut inside: Vec<&Path> = directory
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor.starts_with(root.path()))
            .collect();
        inside.reverse();
        for ancestor in inside {
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
            if let Some(compiled) = self.rules(directory, &directory.join(name)).await? {
                levels.push(compiled);
            }
        }
        Ok(rules)
    }

    /// The rules of the repository's `info/exclude`, relative to its root.
    /// A store that cannot be read holds none: unlike an ignore file in the
    /// workspace, it never fails the scan.
    async fn exclude_rules(&self, repository: &Path) -> Result<Option<Arc<Gitignore>>, ToolError> {
        let Some(git) = git_directory(repository).await else {
            return Ok(None);
        };
        match self.rules(repository, &git.join("info/exclude")).await {
            Err(ToolError::UnsupportedResource(_)) => Ok(None),
            other => other,
        }
    }

    /// The compiled rules of the ignore file `path`, relative to `base`, or
    /// `None` when there is no such file.
    async fn rules(&self, base: &Path, path: &Path) -> Result<Option<Arc<Gitignore>>, ToolError> {
        let Some(text) = self.read(path).await? else {
            return Ok(None);
        };
        let base = base.to_path_buf();
        let compiled = move || compile(base, &text);
        Ok(Some(Arc::new(
            worker::blocking("compile ignore rules", compiled).await?,
        )))
    }

    /// Whether the scan leaves `entry` out: `.git`, or an entry the rule
    /// that decides for it ignores.
    pub(crate) fn ignores(&self, entry: &PathEntry) -> bool {
        if entry.name == ".git" {
            return true;
        }
        let path = self.root.path().join(&entry.path);
        let directory = entry.kind == PathKind::Directory;
        let levels = self.levels.iter().flat_map(|levels| levels.iter().rev());
        for level in levels.chain(&self.exclude) {
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
            ToolError::UnsupportedResource(format!(
                "cannot read the ignore file {}; set include_ignored to scan without ignore files",
                self.shown(path)
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
            return Err(ToolError::LimitExceeded(format!(
                "the ignore file {} exceeds 256 KiB; set include_ignored to scan without ignore files",
                self.shown(path)
            )));
        }
        self.charge(path, bytes.len())?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Take `bytes` of the ignore file `path` from the scan's budget, or
    /// refuse once it is spent.
    fn charge(&self, path: &Path, bytes: usize) -> Result<(), ToolError> {
        let mut left = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
        *left = left.checked_sub(bytes).ok_or_else(|| {
            ToolError::LimitExceeded(format!(
                "ignore files exceed 2 MiB in one scan, at {}; set include_ignored to scan without ignore files",
                self.shown(path)
            ))
        })?;
        Ok(())
    }

    /// How an ignore file is named to the model: relative to the workspace
    /// root, with `..` for a directory above it, never as a host path.
    fn shown(&self, path: &Path) -> String {
        if let Ok(relative) = self.root.relative(path) {
            return relative;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let parent = path.parent().unwrap_or(path);
        match self.root.path().ancestors().position(|up| up == parent) {
            Some(levels) => format!("{}{name}", "../".repeat(levels)),
            // A worktree's `info/exclude` lies in its repository's store.
            None => format!("info/{name}"),
        }
    }
}

/// The nearest of `root` and the directories above it that holds `.git`, a
/// directory or a worktree's file: the root of the repository the workspace
/// is in.
async fn repository(root: &Path) -> Option<PathBuf> {
    for directory in root.ancestors() {
        if fs::symlink_metadata(directory.join(".git")).await.is_ok() {
            return Some(directory.to_path_buf());
        }
    }
    None
}

/// The directory holding `repository`'s shared files, `info/exclude` among
/// them: its `.git` directory, or for a worktree, the directory its `.git`
/// file points to, or that one's `commondir`. `None` when it cannot be read.
///
/// A `.git` file in the workspace is the model's to write, so a pointer is
/// followed only to a Git directory, one holding a regular `HEAD`: files
/// named `commondir` and `info/exclude` elsewhere on the host are not read.
async fn git_directory(repository: &Path) -> Option<PathBuf> {
    let git = repository.join(".git");
    if fs::symlink_metadata(&git).await.ok()?.is_dir() {
        return Some(git);
    }
    let pointer = small_file(&git).await?;
    let own = repository.join(pointer.strip_prefix("gitdir:")?.trim());
    if !holds_head(&own).await {
        return None;
    }
    if let Some(common) = small_file(&own.join("commondir")).await {
        let common = own.join(common.trim());
        if holds_head(&common).await {
            return Some(common);
        }
    }
    Some(own)
}

/// Whether `directory` looks like a Git directory: it holds a regular `HEAD`.
async fn holds_head(directory: &Path) -> bool {
    fs::symlink_metadata(directory.join("HEAD"))
        .await
        .is_ok_and(|info| info.is_file())
}

/// The text of a small file Git writes, or `None`. Only a regular file is
/// opened: opening a FIFO blocks until a writer comes, and would hold a
/// blocking thread for good.
async fn small_file(path: &Path) -> Option<String> {
    if !fs::symlink_metadata(path).await.ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .await
        .ok()?
        .take(MAX_POINTER_BYTES)
        .read_to_string(&mut text)
        .await
        .ok()?;
    Some(text)
}

/// The matcher of one ignore file whose patterns are relative to `base`;
/// blocking.
fn compile(base: PathBuf, text: &str) -> Result<Gitignore, ToolError> {
    let mut builder = GitignoreBuilder::new(base);
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
