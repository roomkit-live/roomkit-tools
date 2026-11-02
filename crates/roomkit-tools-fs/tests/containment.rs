//! Paths outside the workspace are refused before the filesystem is consulted.

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{
    DiffInput, DiffTool, EditInput, EditTool, GlobInput, GlobTool, GrepInput, GrepTool, ListInput,
    ListTool, PatchChange, PatchInput, PatchTool, ReadInput, ReadTool, WriteInput, WriteTool,
};
use tokio_util::sync::CancellationToken;

/// A listing of `path`, Python's `list(path)`: the other arguments default.
fn listing(path: impl Into<String>) -> ListInput {
    ListInput {
        path: path.into(),
        ..ListInput::default()
    }
}

fn refused<T: std::fmt::Debug>(result: &Result<T, ToolError>, path: &str) {
    assert!(
        matches!(result, Err(ToolError::AccessRefused(_))),
        "{path}: {result:?}"
    );
}

/// Existing and missing outside paths must fail the same way, so the error
/// cannot reveal whether something exists outside the workspace.
#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn outside_paths(outside: &std::path::Path) -> Vec<String> {
    std::fs::write(outside.join("present.txt"), "outside\n").unwrap();
    vec![
        outside.join("present.txt").display().to_string(),
        outside.join("missing.txt").display().to_string(),
        outside.join("missing/child.txt").display().to_string(),
        "../missing-sibling/child.txt".into(),
        "nested/../../missing.txt".into(),
    ]
}

#[tokio::test]
async fn read_only_tools_refuse_outside_paths_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("inside.txt"), "inside\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let read = ReadTool::new(root.path()).await.unwrap();
    let list = ListTool::new(root.path()).await.unwrap();
    let glob = GlobTool::new(root.path()).await.unwrap();
    let grep = GrepTool::new(root.path()).await.unwrap();
    let diff = DiffTool::new(root.path()).await.unwrap();
    for path in outside_paths(outside.path()) {
        refused(
            &read.read(ReadInput::new(path.clone()), &cancel).await,
            &path,
        );
        refused(&list.list(listing(path.clone()), &cancel).await, &path);
        let mut input = GlobInput::new("*");
        input.path.clone_from(&path);
        refused(&glob.glob(input, &cancel).await, &path);
        let mut input = GrepInput::new("outside");
        input.path.clone_from(&path);
        refused(&grep.grep(input, &cancel).await, &path);
        refused(
            &diff
                .diff(DiffInput::new("inside.txt", path.clone()), &cancel)
                .await,
            &path,
        );
    }
}

#[tokio::test]
async fn mutation_tools_refuse_outside_paths_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let write = WriteTool::new(root.path()).await.unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let patch = PatchTool::new(root.path()).await.unwrap();
    for path in outside_paths(outside.path()) {
        refused(
            &write
                .write(WriteInput::new(path.clone(), "x"), &cancel)
                .await,
            &path,
        );
        refused(
            &edit
                .edit(EditInput::new(path.clone(), "outside", "x"), &cancel)
                .await,
            &path,
        );
        let change = PatchChange::Update {
            path: path.clone(),
            old_content: "outside\n".into(),
            new_content: "x".into(),
        };
        refused(
            &patch.patch(PatchInput::new(vec![change]), &cancel).await,
            &path,
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.path().join("present.txt")).unwrap(),
        "outside\n"
    );
}

#[tokio::test]
async fn results_name_paths_relative_to_the_workspace_never_the_host_path() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("menus")).unwrap();
    std::fs::write(root.path().join("menus/bread.txt"), "rye\n").unwrap();
    let cancel = CancellationToken::new();
    let absolute = root.path().join("menus/bread.txt").display().to_string();
    let read = ReadTool::new(root.path()).await.unwrap();
    for input in ["menus/bread.txt", "./menus/../menus/bread.txt", &absolute] {
        let page = read.read(ReadInput::new(input), &cancel).await.unwrap();
        assert_eq!(page.data.path, "menus/bread.txt", "{input}");
    }
    let list = ListTool::new(root.path()).await.unwrap();
    for (input, expected) in [(".", "."), ("menus", "menus")] {
        let page = list.list(listing(input), &cancel).await.unwrap();
        assert_eq!(page.data.path, expected);
    }
    let glob = GlobTool::new(root.path()).await.unwrap();
    let mut input = GlobInput::new("*.txt");
    input.path = "menus".into();
    assert_eq!(glob.glob(input, &cancel).await.unwrap().data.path, "menus");
    let grep = GrepTool::new(root.path()).await.unwrap();
    let page = grep.grep(GrepInput::new("rye"), &cancel).await.unwrap();
    assert_eq!(page.data.path, ".");
    let mut input = GrepInput::new("rye");
    input.path = absolute;
    let page = grep.grep(input, &cancel).await.unwrap();
    assert_eq!(page.data.path, "menus/bread.txt");
    assert_eq!(page.data.matches[0].path, "menus/bread.txt");
}

#[cfg(unix)]
#[tokio::test]
async fn absolute_paths_through_the_hosts_root_spelling_stay_accepted() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("real");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("notes.txt"), "hello\n").unwrap();
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let read = ReadTool::new(&link).await.unwrap();
    let output = read
        .read(
            ReadInput::new(link.join("notes.txt").display().to_string()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(output.text, "1: hello\n");
    refused(
        &read
            .read(
                ReadInput::new(parent.path().join("missing").display().to_string()),
                &CancellationToken::new(),
            )
            .await,
        "missing sibling of the root",
    );
}

/// With `m` a symlink to `real/sub`, the root spelled `m/../ws` is `real/ws`,
/// so the lexically similar `ws` beside `m` is an unrelated directory and
/// must not be accepted as the root's spelling.
#[cfg(unix)]
#[tokio::test]
async fn a_root_spelling_with_dot_dot_after_a_symlink_is_not_an_alias() {
    let parent = tempfile::tempdir().unwrap();
    let base = parent.path().canonicalize().unwrap();
    for directory in ["real/ws", "real/sub", "ws"] {
        std::fs::create_dir_all(base.join(directory)).unwrap();
    }
    std::fs::write(base.join("real/ws/notes.txt"), "inside\n").unwrap();
    std::fs::write(base.join("ws/present.txt"), "unrelated\n").unwrap();
    std::os::unix::fs::symlink(base.join("real/sub"), base.join("m")).unwrap();
    let read = ReadTool::new(base.join("m/../ws")).await.unwrap();
    let cancel = CancellationToken::new();
    let page = read.read(ReadInput::new("notes.txt"), &cancel).await;
    assert_eq!(page.unwrap().text, "1: inside\n");
    for name in ["present.txt", "missing.txt"] {
        let path = base.join("ws").join(name).display().to_string();
        refused(
            &read.read(ReadInput::new(path.clone()), &cancel).await,
            &path,
        );
    }
}

/// A path that leaves the root and comes back is refused before any lookup,
/// so the directories it passes through cannot be probed for existence.
#[tokio::test]
async fn paths_that_leave_the_root_and_come_back_are_refused() {
    let parent = tempfile::tempdir().unwrap();
    let base = parent.path().canonicalize().unwrap();
    let root = base.join("ws");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir(base.join("exists_dir")).unwrap();
    std::fs::write(root.join("a.txt"), "inside\n").unwrap();
    let detours = |name: &str| {
        [
            format!("../exists_dir/../ws/{name}"),
            format!("../missing_dir/../ws/{name}"),
            format!("sub/../../ws/{name}"),
            format!("{}/exists_dir/../ws/{name}", base.display()),
            format!("{}/missing_dir/../ws/{name}", base.display()),
            format!("{}/../ws/{name}", root.display()),
        ]
    };
    let cancel = CancellationToken::new();
    let read = ReadTool::new(&root).await.unwrap();
    let list = ListTool::new(&root).await.unwrap();
    let write = WriteTool::new(&root).await.unwrap();
    let edit = EditTool::new(&root).await.unwrap();
    for path in detours("a.txt") {
        refused(
            &read.read(ReadInput::new(path.clone()), &cancel).await,
            &path,
        );
        refused(
            &edit
                .edit(EditInput::new(path.clone(), "inside", "x"), &cancel)
                .await,
            &path,
        );
    }
    for path in detours("sub") {
        refused(&list.list(listing(path.clone()), &cancel).await, &path);
    }
    for path in detours("new.txt") {
        refused(
            &write
                .write(WriteInput::new(path.clone(), "x"), &cancel)
                .await,
            &path,
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "inside\n"
    );
    assert!(!root.join("new.txt").exists());
}

/// Beneath a workspace symlink to an outside directory, every path is
/// refused, so the error does not reveal what exists out there.
#[cfg(unix)]
#[tokio::test]
async fn paths_beneath_a_symlink_to_outside_are_refused_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("inside.txt"), "inside\n").unwrap();
    std::fs::create_dir(outside.path().join("dir")).unwrap();
    std::fs::write(outside.path().join("present.txt"), "outside\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
    let cancel = CancellationToken::new();
    let read = ReadTool::new(root.path()).await.unwrap();
    let list = ListTool::new(root.path()).await.unwrap();
    let grep = GrepTool::new(root.path()).await.unwrap();
    let diff = DiffTool::new(root.path()).await.unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let patch = PatchTool::new(root.path()).await.unwrap();
    for path in [
        "link/present.txt",
        "link/missing.txt",
        "link/dir/child.txt",
        "link/missing/child.txt",
        "link/../missing.txt",
    ] {
        refused(&read.read(ReadInput::new(path), &cancel).await, path);
        refused(&list.list(listing(path), &cancel).await, path);
        let mut input = GrepInput::new("outside");
        input.path = path.into();
        refused(&grep.grep(input, &cancel).await, path);
        refused(
            &diff.diff(DiffInput::new("inside.txt", path), &cancel).await,
            path,
        );
        refused(
            &edit
                .edit(EditInput::new(path, "outside", "x"), &cancel)
                .await,
            path,
        );
        let change = PatchChange::Update {
            path: path.into(),
            old_content: "outside\n".into(),
            new_content: "x".into(),
        };
        refused(
            &patch.patch(PatchInput::new(vec![change]), &cancel).await,
            path,
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.path().join("present.txt")).unwrap(),
        "outside\n"
    );
    // Beneath a symlink that stays inside, a missing path is not refused.
    std::fs::create_dir(root.path().join("sub")).unwrap();
    std::os::unix::fs::symlink(root.path().join("sub"), root.path().join("inner")).unwrap();
    for path in ["inner/missing.txt", "inner/missing/child.txt"] {
        let result = read.read(ReadInput::new(path), &cancel).await;
        assert!(
            matches!(result, Err(ToolError::Io { .. })),
            "{path}: {result:?}"
        );
    }
}
