//! Read-only comparisons and real structured patch effects.

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{
    DiffInput, DiffLimits, DiffTool, EditInput, EditTool, PatchChange, PatchInput, PatchLimits,
    PatchOperation, PatchTool, Workspace,
};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "fixture failures must fail the test")]
async fn comparison(
    old: &str,
    new: &str,
    context: usize,
) -> roomkit_tools_core::Output<roomkit_tools_fs::DiffResult> {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("before"), old).unwrap();
    std::fs::write(root.path().join("after"), new).unwrap();
    let mut input = DiffInput::new("before", "after");
    input.context_lines = context;
    let result = DiffTool::new(root.path())
        .await
        .unwrap()
        .diff(input, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("before")).unwrap(),
        old
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("after")).unwrap(),
        new
    );
    result
}

#[tokio::test]
async fn diff_reports_actual_changes_and_disjoint_hunks_without_writing() {
    let result = comparison(
        "a\nb\nc\nd\ne\nf\ng\nh\ni\n",
        "a\nB\nc\nd\ne\nf\ng\nH\ni\n",
        1,
    )
    .await;
    assert_eq!(result.data.additions, 2);
    assert_eq!(result.data.deletions, 2);
    assert_eq!(result.data.hunks.len(), 2);
    assert_eq!(result.data.hunks[0].old_start, 1);
    assert_eq!(result.data.hunks[1].new_start, 7);
    assert!(result.text.contains("-b\n+B\n"));
    assert!(result.text.contains("-h\n+H\n"));
    let equal = comparison("same\n", "same\n", 3).await;
    assert!(!equal.data.changed);
    assert_eq!(equal.data.hunks, []);
    assert_eq!(equal.text, "");
}

#[tokio::test]
async fn diff_handles_empty_versions_and_context_zero() {
    let added = comparison("", "a\nb\n", 0).await;
    assert!(added.text.contains("@@ -0,0 +1,2 @@"));
    assert_eq!(added.data.additions, 2);
    let removed = comparison("a\nb\n", "", 0).await;
    assert!(removed.text.contains("@@ -1,2 +0,0 @@"));
    let tail = comparison("a\n", "a\nb\n", 0).await;
    assert!(tail.text.contains("@@ -1,0 +2,1 @@"));
}

#[tokio::test]
async fn diff_preserves_unicode_bom_line_endings_and_missing_final_newlines() {
    let result = comparison("\u{feff}πxπ\r\nend", "\u{feff}πxπ\nend\n", 3).await;
    assert_eq!(result.data.additions, 2);
    assert_eq!(result.data.deletions, 2);
    assert!(result.text.contains("\\ No newline at end of file"));
    assert!(
        result.data.hunks[0]
            .lines
            .iter()
            .any(|line| line.text == "\u{feff}πxπ\r\n")
    );
}

#[tokio::test]
async fn diff_enforces_file_line_complexity_and_complete_output_bounds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "a\nb\nc\n").unwrap();
    std::fs::write(root.path().join("b"), "d\ne\nf\n").unwrap();
    let tool = DiffTool::new(root.path()).await.unwrap();
    let bounds = [
        DiffLimits {
            max_file_bytes: 2,
            ..DiffLimits::default()
        },
        DiffLimits {
            max_lines: 2,
            ..DiffLimits::default()
        },
        DiffLimits {
            max_comparison_cells: 8,
            ..DiffLimits::default()
        },
        DiffLimits {
            max_output_bytes: 5,
            ..DiffLimits::default()
        },
    ];
    for limits in bounds {
        let result = tool
            .clone()
            .with_limits(limits)
            .unwrap()
            .diff(DiffInput::new("a", "b"), &CancellationToken::new())
            .await;
        assert!(matches!(result, Err(ToolError::LimitExceeded(_))));
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        tool.diff(DiffInput::new("a", "b"), &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(
        serde_json::from_str::<DiffInput>(r#"{"old_path":"a","new_path":"b","unknown":1}"#)
            .is_err()
    );
}

#[tokio::test]
async fn diff_rejects_binary_directories_missing_files_and_outside_roots() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "text").unwrap();
    std::fs::write(root.path().join("binary"), [0, 255]).unwrap();
    std::fs::write(outside.path().join("file"), "outside").unwrap();
    let tool = DiffTool::new(root.path()).await.unwrap();
    for name in ["binary", "."] {
        assert!(matches!(
            tool.diff(DiffInput::new("a", name), &CancellationToken::new())
                .await,
            Err(ToolError::UnsupportedResource(_))
        ));
    }
    assert!(matches!(
        tool.diff(DiffInput::new("a", "missing"), &CancellationToken::new())
            .await,
        Err(ToolError::Io { .. })
    ));
    assert!(matches!(
        tool.diff(
            DiffInput::new("a", outside.path().join("file").display().to_string()),
            &CancellationToken::new()
        )
        .await,
        Err(ToolError::AccessRefused(_))
    ));
}

fn update(path: &str, old: &str, new: &str) -> PatchChange {
    PatchChange::Update {
        path: path.into(),
        old_content: old.into(),
        new_content: new.into(),
    }
}

#[tokio::test]
async fn patch_previews_then_applies_add_update_delete_and_move() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hours"), "Saturday: 16:00\n").unwrap();
    std::fs::write(root.path().join("obsolete"), "old\n").unwrap();
    std::fs::write(root.path().join("orders"), "43 | Taylor\n").unwrap();
    let tool = PatchTool::new(root.path()).await.unwrap();
    let mut input = PatchInput::new(vec![
        update("hours", "Saturday: 16:00\n", "Saturday: 17:00\n"),
        PatchChange::Add {
            path: "notice".into(),
            content: "New hours\n".into(),
        },
        PatchChange::Delete {
            path: "obsolete".into(),
            old_content: "old\n".into(),
        },
        PatchChange::Move {
            from: "orders".into(),
            to: "archived-orders".into(),
            old_content: "43 | Taylor\n".into(),
            new_content: None,
        },
    ]);
    let preview = tool
        .patch(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert!(preview.data.dry_run && preview.data.completed && preview.data.applied.is_empty());
    assert_eq!(preview.data.previews.len(), 4);
    assert_eq!(
        std::fs::read_to_string(root.path().join("hours")).unwrap(),
        "Saturday: 16:00\n"
    );
    assert!(!root.path().join("notice").exists());
    assert!(root.path().join("obsolete").exists());
    input.dry_run = false;
    let result = tool.patch(input, &CancellationToken::new()).await.unwrap();
    assert!(result.data.completed);
    assert_eq!(result.data.applied.len(), 5);
    assert_eq!(result.data.applied[3].operation, PatchOperation::Add);
    assert_eq!(result.data.applied[4].operation, PatchOperation::Delete);
    assert_eq!(
        std::fs::read_to_string(root.path().join("hours")).unwrap(),
        "Saturday: 17:00\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("notice")).unwrap(),
        "New hours\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("archived-orders")).unwrap(),
        "43 | Taylor\n"
    );
    assert!(!root.path().join("orders").exists());
    assert!(!root.path().join("obsolete").exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
}

#[tokio::test]
async fn patch_validates_every_source_before_any_effect_and_rechecks_after_preview() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "a").unwrap();
    std::fs::write(root.path().join("b"), "b").unwrap();
    let tool = PatchTool::new(root.path()).await.unwrap();
    let mut input = PatchInput::new(vec![update("a", "a", "A"), update("b", "wrong", "B")]);
    input.dry_run = false;
    assert!(matches!(
        tool.patch(input, &CancellationToken::new()).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(std::fs::read_to_string(root.path().join("a")).unwrap(), "a");
    let mut input = PatchInput::new(vec![update("a", "a", "A")]);
    tool.patch(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    std::fs::write(root.path().join("a"), "external").unwrap();
    input.dry_run = false;
    assert!(matches!(
        tool.patch(input, &CancellationToken::new()).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("a")).unwrap(),
        "external"
    );
}

#[tokio::test]
async fn patch_accepts_lf_content_for_files_with_uniform_crlf_endings() {
    let root = tempfile::tempdir().unwrap();
    for name in ["update", "delete", "move", "rewrite"] {
        std::fs::write(root.path().join(name), "one\r\ntwo\r\n").unwrap();
    }
    std::fs::write(root.path().join("mixed"), "one\r\ntwo\n").unwrap();
    let tool = PatchTool::new(root.path()).await.unwrap();
    let mut input = PatchInput::new(vec![
        update("update", "one\ntwo\n", "one\n2\n"),
        PatchChange::Delete {
            path: "delete".into(),
            old_content: "one\ntwo\n".into(),
        },
        PatchChange::Move {
            from: "move".into(),
            to: "moved".into(),
            old_content: "one\ntwo\n".into(),
            new_content: None,
        },
        PatchChange::Move {
            from: "rewrite".into(),
            to: "rewritten".into(),
            old_content: "one\ntwo\n".into(),
            new_content: Some("three\n".into()),
        },
    ]);
    input.dry_run = false;
    let result = tool.patch(input, &CancellationToken::new()).await.unwrap();
    assert!(result.data.completed);
    let read = |name: &str| std::fs::read_to_string(root.path().join(name)).unwrap();
    assert_eq!(read("update"), "one\r\n2\r\n");
    assert!(!root.path().join("delete").exists());
    assert_eq!(read("moved"), "one\r\ntwo\r\n");
    assert_eq!(read("rewritten"), "three\r\n");
    let mut input = PatchInput::new(vec![update("mixed", "one\ntwo\n", "x")]);
    input.dry_run = false;
    assert!(matches!(
        tool.patch(input, &CancellationToken::new()).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(read("mixed"), "one\r\ntwo\n");
}

#[tokio::test]
async fn patch_preserves_exact_bytes_and_noops_and_move_can_change_text() {
    let root = tempfile::tempdir().unwrap();
    let source = "\u{feff}πxπ\r\nend";
    std::fs::write(root.path().join("a"), source).unwrap();
    let tool = PatchTool::new(root.path()).await.unwrap();
    let mut input = PatchInput::new(vec![update("a", source, source)]);
    input.dry_run = false;
    let no_op = tool.patch(input, &CancellationToken::new()).await.unwrap();
    assert!(no_op.data.completed && no_op.data.applied.is_empty());
    assert!(!no_op.data.previews[0].changed);
    let mut input = PatchInput::new(vec![PatchChange::Move {
        from: "a".into(),
        to: "b".into(),
        old_content: source.into(),
        new_content: Some("\u{feff}πxπ\r\nEND".into()),
    }]);
    input.dry_run = false;
    tool.patch(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("b")).unwrap(),
        "\u{feff}πxπ\r\nEND"
    );
}

#[tokio::test]
async fn patch_rejects_overlaps_existing_targets_and_missing_parents() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "a").unwrap();
    std::fs::write(root.path().join("b"), "b").unwrap();
    let tool = PatchTool::new(root.path()).await.unwrap();
    let scenarios = vec![
        vec![update("a", "a", "A"), update("./a", "a", "B")],
        vec![PatchChange::Add {
            path: "a".into(),
            content: "overwrite".into(),
        }],
        vec![PatchChange::Add {
            path: "missing/file".into(),
            content: "x".into(),
        }],
        vec![PatchChange::Move {
            from: "a".into(),
            to: "b".into(),
            old_content: "a".into(),
            new_content: None,
        }],
        vec![PatchChange::Delete {
            path: "../outside".into(),
            old_content: "x".into(),
        }],
    ];
    for changes in scenarios {
        let mut input = PatchInput::new(changes);
        input.dry_run = false;
        assert!(tool.patch(input, &CancellationToken::new()).await.is_err());
        assert_eq!(std::fs::read_to_string(root.path().join("a")).unwrap(), "a");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}

#[tokio::test]
async fn patch_limits_and_cancellation_prevent_writes() {
    let root = tempfile::tempdir().unwrap();
    let mut input = PatchInput::new(vec![PatchChange::Add {
        path: "new".into(),
        content: "some text\n".into(),
    }]);
    input.dry_run = false;
    let tool = PatchTool::new(root.path()).await.unwrap();
    let bounds = [
        PatchLimits {
            max_input_bytes: 1,
            ..PatchLimits::default()
        },
        PatchLimits {
            diff: DiffLimits {
                max_file_bytes: 1,
                ..DiffLimits::default()
            },
            ..PatchLimits::default()
        },
        PatchLimits {
            diff: DiffLimits {
                max_output_bytes: 1,
                ..DiffLimits::default()
            },
            ..PatchLimits::default()
        },
    ];
    for limits in bounds {
        assert!(matches!(
            tool.clone()
                .with_limits(limits)
                .unwrap()
                .patch(input.clone(), &CancellationToken::new())
                .await,
            Err(ToolError::LimitExceeded(_))
        ));
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        tool.patch(input, &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let input: PatchInput =
        serde_json::from_str(r#"{"changes":[{"op":"add","path":"new","content":"x"}]}"#).unwrap();
    assert!(input.dry_run);
    assert!(
        serde_json::from_str::<PatchInput>(r#"{"changes":[{"op":"delete","path":"a"}]}"#).is_err()
    );
    assert!(
        serde_json::from_str::<PatchInput>(
            r#"{"changes":[{"op":"add","path":"new","content":"x","extra":true}]}"#
        )
        .is_err()
    );
}

#[tokio::test]
async fn patch_and_edit_share_the_same_mutation_scope() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("orders"), "old").unwrap();
    let workspace = Workspace::new(root.path()).await.unwrap();
    let patch = PatchTool::from_workspace(workspace.clone());
    let edit = EditTool::from_workspace(workspace);
    let mut input = PatchInput::new(vec![update("orders", "old", "patch")]);
    input.dry_run = false;
    let token = CancellationToken::new();
    let (patched, edited) = tokio::join!(
        patch.patch(input, &token),
        edit.edit(EditInput::new("orders", "old", "edit"), &token)
    );
    assert_ne!(patched.is_ok(), edited.is_ok());
    let actual = std::fs::read_to_string(root.path().join("orders")).unwrap();
    assert_eq!(actual, if patched.is_ok() { "patch" } else { "edit" });
}

#[cfg(unix)]
#[tokio::test]
async fn diff_allows_internal_symlinks_but_patch_refuses_leaf_symlinks_and_preserves_move_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source"), "x").unwrap();
    symlink("source", root.path().join("link")).unwrap();
    let diff = DiffTool::new(root.path()).await.unwrap();
    assert!(
        !diff
            .diff(DiffInput::new("source", "link"), &CancellationToken::new())
            .await
            .unwrap()
            .data
            .changed
    );
    let patch = PatchTool::new(root.path()).await.unwrap();
    let input = PatchInput::new(vec![update("link", "x", "y")]);
    assert!(matches!(
        patch.patch(input, &CancellationToken::new()).await,
        Err(ToolError::AccessRefused(_))
    ));
    std::fs::set_permissions(
        root.path().join("source"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    let mut input = PatchInput::new(vec![PatchChange::Move {
        from: "source".into(),
        to: "dest".into(),
        old_content: "x".into(),
        new_content: None,
    }]);
    input.dry_run = false;
    patch.patch(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(
        std::fs::metadata(root.path().join("dest"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o750
    );
}
