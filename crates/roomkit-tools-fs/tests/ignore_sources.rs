//! Where a scan's ignore rules come from besides the workspace's own
//! `.gitignore` and `.ignore` files: the repository's `info/exclude`, and
//! ignore files above the workspace up to the repository's root.

use std::path::Path;

use roomkit_tools_fs::{GlobInput, GlobTool, GrepInput, GrepTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[expect(clippy::unwrap_used, reason = "the search must succeed")]
async fn globbed(root: &Path, pattern: &str) -> Vec<String> {
    let found = GlobTool::new(root)
        .await
        .unwrap()
        .glob(GlobInput::new(pattern), &CancellationToken::new())
        .await
        .unwrap();
    found
        .data
        .entries
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

/// `.git/info/exclude` leaves entries out as `.gitignore` does.
#[tokio::test]
async fn the_repository_s_info_exclude_leaves_entries_out() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".git/info/exclude"), "target/\n");
    write(&root.path().join("src/lib.rs"), "fn note() {}\n");
    write(&root.path().join("target/debug/build.rs"), "fn note() {}\n");
    assert_eq!(globbed(root.path(), "**/*.rs").await, ["src/lib.rs"]);
    let found = GrepTool::new(root.path())
        .await
        .unwrap()
        .grep(GrepInput::new("note"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(found.data.matches.len(), 1);
}

/// A workspace below a repository's root applies the ignore files of the
/// directories between them, and none above the repository.
#[tokio::test]
async fn ignore_files_above_the_workspace_apply_up_to_its_repository() {
    let outer = tempfile::tempdir().unwrap();
    let repository = outer.path().join("repository");
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    write(&outer.path().join(".gitignore"), "*.rs\n");
    write(&repository.join(".gitignore"), "*.log\n/app/build/\n");
    write(&repository.join("app/.ignore"), "!keep.log\n");
    for file in ["main.rs", "debug.log", "keep.log", "build/out.rs"] {
        write(&repository.join("app").join(file), "x\n");
    }
    assert_eq!(
        globbed(&repository.join("app"), "**").await,
        [".ignore", "keep.log", "main.rs"]
    );
}

/// A worktree's `.git` is a file naming its directory in the repository's
/// store, whose `commondir` holds the shared `info/exclude`.
#[tokio::test]
async fn a_worktree_reads_its_repository_s_info_exclude() {
    let outer = tempfile::tempdir().unwrap();
    let store = outer.path().join("main/.git");
    write(&store.join("info/exclude"), "*.tmp\n");
    write(&store.join("worktrees/feature/commondir"), "../..\n");
    let worktree = outer.path().join("feature");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", store.join("worktrees/feature").display()),
    );
    write(&worktree.join("notes.tmp"), "x\n");
    write(&worktree.join("notes.txt"), "x\n");
    assert_eq!(globbed(&worktree, "notes.*").await, ["notes.txt"]);
}
