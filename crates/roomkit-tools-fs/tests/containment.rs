//! Paths outside the workspace are refused before the filesystem is consulted.

use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    DiffInput, DiffTool, EditInput, EditTool, GlobInput, GlobTool, GrepInput, GrepTool, ListInput,
    ListTool, PatchChange, PatchInput, PatchTool, ReadInput, ReadTool, WriteInput, WriteTool,
};
use tokio_util::sync::CancellationToken;

fn refused<T: std::fmt::Debug>(result: &Result<T, Error>, path: &str) {
    assert!(
        matches!(result, Err(Error::Refused(_))),
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
        refused(
            &list.list(ListInput::new(path.clone()), &cancel).await,
            &path,
        );
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
