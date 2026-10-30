//! Mutation outcomes, unchanged bytes on refusal and shared serialization.

use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    EditInput, EditLimits, EditTool, MutationWorkspace, WriteInput, WriteLimits, WriteTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn write_creates_exact_unicode_bytes_and_requires_explicit_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let tool = WriteTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let content = "\u{feff}Order 43 : logπ\r\n";
    let input = WriteInput::new("orders.txt", content);
    let result = tool.write(input.clone(), &cancel).await.unwrap();
    assert!(result.data.created && result.data.changed);
    assert_eq!(result.data.path, "orders.txt");
    assert_eq!(result.data.bytes_written, content.len());
    assert_eq!(
        std::fs::read(root.path().join("orders.txt")).unwrap(),
        content.as_bytes()
    );
    assert!(matches!(
        tool.write(input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    let mut input = input;
    input.overwrite = true;
    let unchanged = tool.write(input.clone(), &cancel).await.unwrap();
    assert!(!unchanged.data.created && !unchanged.data.changed);
    input.content = String::new();
    let empty = tool.write(input, &cancel).await.unwrap();
    assert!(!empty.data.created && empty.data.changed);
    assert_eq!(empty.data.bytes_written, 0);
    assert_eq!(std::fs::read(root.path().join("orders.txt")).unwrap(), b"");
}

#[tokio::test]
async fn edit_preserves_bom_crlf_and_all_unmatched_text() {
    let root = tempfile::tempdir().unwrap();
    let before = "\u{feff}Orders\r\n43 | Taylor | 2 notebooks\r\n44 | Alex | logπ\r\n";
    std::fs::write(root.path().join("orders.txt"), before).unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let result = edit
        .edit(
            EditInput::new(
                "orders.txt",
                "43 | Taylor | 2 notebooks",
                "43 | Taylor | 3 notebooks",
            ),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(result.data.replacements, 1);
    assert!(result.data.changed);
    assert_eq!(result.data.bytes_before, before.len());
    assert_eq!(result.data.bytes_after, before.len());
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders.txt")).unwrap(),
        before.replace("2 notebooks", "3 notebooks")
    );
    edit.edit(
        EditInput::new("orders.txt", "44 | Alex | logπ\r\n", ""),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders.txt")).unwrap(),
        "\u{feff}Orders\r\n43 | Taylor | 3 notebooks\r\n"
    );
}

#[tokio::test]
async fn edit_refuses_empty_missing_and_ambiguous_matches_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let content = "notebook\nnotebook\n";
    std::fs::write(root.path().join("orders"), content).unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    for old in ["", "pen", "notebook", "notebook\r\n"] {
        assert!(matches!(
            edit.edit(
                EditInput::new("orders", old, "bread"),
                &CancellationToken::new()
            )
            .await,
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.path().join("orders")).unwrap(),
            content
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn replace_all_is_explicit_and_uses_non_overlapping_matches() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("orders"), "aaaaa").unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let mut input = EditInput::new("orders", "aa", "π");
    input.replace_all = true;
    let result = edit.edit(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(result.data.replacements, 2);
    assert_eq!(result.data.bytes_after, "ππa".len());
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "ππa"
    );
    let result = edit
        .edit(
            EditInput::new("orders", "ππa", "ππa"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.data.changed);
    edit.edit(
        EditInput::new("orders", "ππa", ""),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(root.path().join("orders").is_file());
    assert_eq!(std::fs::read(root.path().join("orders")).unwrap(), b"");
}

#[tokio::test]
async fn bounds_cover_existing_input_and_expanded_content_before_writing() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("orders"), "aaa").unwrap();
    let mut limits = WriteLimits::default();
    limits.max_file_bytes = 2;
    let write = WriteTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let cancel = CancellationToken::new();
    assert!(matches!(
        write.write(WriteInput::new("new", "ππ"), &cancel).await,
        Err(Error::LimitExceeded(_))
    ));
    let mut overwrite = WriteInput::new("orders", "a");
    overwrite.overwrite = true;
    assert!(matches!(
        write.write(overwrite, &cancel).await,
        Err(Error::LimitExceeded(_))
    ));
    let mut limits = EditLimits::default();
    limits.max_file_bytes = 4;
    let edit = EditTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = EditInput::new("orders", "a", "xx");
    input.replace_all = true;
    assert!(matches!(
        edit.edit(input, &cancel).await,
        Err(Error::LimitExceeded(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "aaa"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn missing_parents_are_created_and_non_files_are_not_replaced() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("directory")).unwrap();
    let write = WriteTool::new(root.path()).await.unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let result = write
        .write(WriteInput::new("missing/nested/orders", "a"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        result.data.created_directories,
        ["missing", "missing/nested"]
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("missing/nested/orders")).unwrap(),
        "a"
    );
    assert!(matches!(
        edit.edit(EditInput::new("absent", "a", "b"), &cancel).await,
        Err(Error::Io { .. })
    ));
    assert!(matches!(
        write
            .write(WriteInput::new("directory", "a"), &cancel)
            .await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        edit.edit(EditInput::new("directory", "a", "b"), &cancel)
            .await,
        Err(Error::Unsupported(_))
    ));
    for path in ["", ".", "directory/.", "directory/", "directory/.."] {
        assert!(matches!(
            write.write(WriteInput::new(path, "a"), &cancel).await,
            Err(Error::InvalidInput(_))
        ));
    }
    assert!(root.path().join("directory").is_dir());
}

#[tokio::test]
async fn rejected_writes_do_not_create_parents() {
    let root = tempfile::tempdir().unwrap();
    let tool = WriteTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut stale = WriteInput::new("new/nested/file", "data");
    stale.expected_revision = Some(format!("sha256:{}", "0".repeat(64)));
    for input in [
        stale,
        WriteInput::new("new/nested/file", "bad\0text"),
        WriteInput::new("new/../../outside/file", "data"),
    ] {
        assert!(tool.write(input, &cancel).await.is_err());
        assert!(!root.path().join("new").exists());
    }
    let deep = format!("{}file", "a/".repeat(65));
    assert!(matches!(
        tool.write(WriteInput::new(deep, "data"), &cancel).await,
        Err(Error::LimitExceeded(_))
    ));
    cancel.cancel();
    assert!(matches!(
        tool.write(WriteInput::new("new/file", "data"), &cancel)
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn missing_parents_do_not_escape_through_symlinks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    symlink(root.path().join("absent"), root.path().join("dangling")).unwrap();
    let tool = WriteTool::new(root.path()).await.unwrap();
    for path in ["escape/new/file", "dangling/new/file"] {
        assert!(
            tool.write(WriteInput::new(path, "data"), &CancellationToken::new())
                .await
                .is_err()
        );
    }
    assert!(!outside.path().join("new").exists());
    assert!(!root.path().join("absent").exists());
    std::fs::create_dir(root.path().join("inside")).unwrap();
    symlink(root.path().join("inside"), root.path().join("alias")).unwrap();
    let result = tool
        .write(
            WriteInput::new("alias/new/file", "data"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.data.path, "inside/new/file");
    assert_eq!(result.data.created_directories, ["inside/new"]);
}

#[tokio::test]
async fn mutation_paths_stay_inside_the_root_and_accept_internal_absolute_paths() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(parent.path().join("secret"), "secret").unwrap();
    let write = WriteTool::new(&root).await.unwrap();
    let edit = EditTool::new(&root).await.unwrap();
    let cancel = CancellationToken::new();
    for path in [
        "../secret".to_owned(),
        parent.path().join("new").to_str().unwrap().to_owned(),
    ] {
        assert!(matches!(
            write.write(WriteInput::new(&path, "oops"), &cancel).await,
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            edit.edit(EditInput::new(path, "secret", "oops"), &cancel)
                .await,
            Err(Error::Refused(_))
        ));
    }
    write
        .write(
            WriteInput::new(root.join("orders").to_str().unwrap(), "good"),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(parent.path().join("secret")).unwrap(),
        "secret"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn leaf_symlinks_are_refused_even_inside_the_root_or_dangling() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("inside")).unwrap();
    std::fs::write(root.path().join("orders"), "safe").unwrap();
    for (target, link) in [
        (root.path().join("orders"), "alias"),
        (root.path().join("absent"), "dangling"),
        (outside.path().to_path_buf(), "outside"),
        (root.path().join("inside"), "internal"),
    ] {
        std::os::unix::fs::symlink(target, root.path().join(link)).unwrap();
    }
    let write = WriteTool::new(root.path()).await.unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    for path in ["alias", "dangling", "outside/order.txt"] {
        let mut input = WriteInput::new(path, "oops");
        input.overwrite = true;
        assert!(matches!(
            write.write(input, &cancel).await,
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            edit.edit(EditInput::new(path, "safe", "oops"), &cancel)
                .await,
            Err(Error::Refused(_))
        ));
    }
    let result = write
        .write(WriteInput::new("internal/order.txt", "yes"), &cancel)
        .await
        .unwrap();
    assert_eq!(result.data.path, "inside/order.txt");
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "safe"
    );
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn binary_content_is_refused_and_pre_cancellation_has_no_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let write = WriteTool::new(root.path()).await.unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    assert!(matches!(
        write.write(WriteInput::new("new", "a\0b"), &cancel).await,
        Err(Error::Unsupported(_))
    ));
    for bytes in [b"a\0b".as_slice(), b"\xff".as_slice()] {
        std::fs::write(root.path().join("orders"), bytes).unwrap();
        let mut input = WriteInput::new("orders", "safe");
        input.overwrite = true;
        assert!(matches!(
            write.write(input, &cancel).await,
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            edit.edit(EditInput::new("orders", "a", "b"), &cancel).await,
            Err(Error::Unsupported(_))
        ));
        assert_eq!(std::fs::read(root.path().join("orders")).unwrap(), bytes);
    }
    cancel.cancel();
    assert!(matches!(
        write.write(WriteInput::new("new", "safe"), &cancel).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        edit.edit(EditInput::new("orders", "a", "b"), &cancel).await,
        Err(Error::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn replacement_preserves_permissions_and_refuses_read_only_files() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("script");
    std::fs::write(&path, "a").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let write = WriteTool::new(root.path()).await.unwrap();
    let mut input = WriteInput::new("script", "b");
    input.overwrite = true;
    write.write(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o750
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440)).unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    assert!(matches!(
        edit.edit(
            EditInput::new("script", "b", "c"),
            &CancellationToken::new()
        )
        .await,
        Err(Error::Refused(_))
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "b");
}

#[tokio::test]
async fn shared_workspace_serializes_mutations_without_losing_independent_edits() {
    let root = tempfile::tempdir().unwrap();
    let workspace = MutationWorkspace::new(root.path()).await.unwrap();
    let write = WriteTool::from_workspace(workspace.clone());
    let edit = EditTool::from_workspace(workspace);
    let cancel = CancellationToken::new();
    write
        .write(
            WriteInput::new("orders", "43: pending\n44: pending\n"),
            &cancel,
        )
        .await
        .unwrap();
    let other = edit.clone();
    let (first, second) = tokio::join!(
        edit.edit(
            EditInput::new("orders", "43: pending", "43: ready"),
            &cancel
        ),
        other.edit(
            EditInput::new("orders", "44: pending", "44: ready"),
            &cancel
        )
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "43: ready\n44: ready\n"
    );
    let (first, second) = tokio::join!(
        write.write(WriteInput::new("new", "first"), &cancel),
        write.write(WriteInput::new("new", "second"), &cancel)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn schemas_require_content_and_replacement_text_and_default_to_safe_modes() {
    let write: WriteInput = serde_json::from_str(r#"{"path":"orders","content":""}"#).unwrap();
    assert!(!write.overwrite);
    let edit: EditInput =
        serde_json::from_str(r#"{"path":"orders","old_text":"a","new_text":""}"#).unwrap();
    assert!(!edit.replace_all);
    assert!(serde_json::from_str::<WriteInput>(r#"{"path":"orders"}"#).is_err());
    assert!(serde_json::from_str::<EditInput>(r#"{"path":"orders","old_text":"a"}"#).is_err());
    assert!(
        serde_json::from_str::<WriteInput>(r#"{"path":"orders","content":"","append":true}"#)
            .is_err()
    );
    for definition in [WriteTool::definition(), EditTool::definition()] {
        assert_eq!(definition.input_schema["additionalProperties"], false);
    }
}
