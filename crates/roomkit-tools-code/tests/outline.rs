//! File boundaries, revision-consistent paging and resource contracts.
use std::num::NonZeroUsize;

use roomkit_tools_code::{CodeLanguage, CodeOutlineInput, CodeOutlineLimits, CodeOutlineTool};
use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn revisions_detect_changes_and_output_is_bounded_without_losing_symbols() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("code.py");
    let source = "class Service:\n    def one(self):\n        return 1\n    def two(self):\n        return 2\n";
    std::fs::write(&path, source).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let mut input = CodeOutlineInput::new("code.py");
    input.limit = NonZeroUsize::new(1);
    let first = tool.outline(input.clone(), &token).await.unwrap();
    assert_eq!(first.data.next_offset, Some(2));
    let mut limits = CodeOutlineLimits::default();
    limits.max_output_bytes = serde_json::to_vec(&first).unwrap().len() + 20;
    let small = tool.clone().with_limits(limits).unwrap();
    input.limit = None;
    let page = small.outline(input.clone(), &token).await.unwrap();
    assert_eq!(page.data.symbols.len(), 1);
    assert!(serde_json::to_vec(&page).unwrap().len() <= limits.max_output_bytes);
    input.expected_revision = Some(page.data.revision);
    input.offset = NonZeroUsize::new(2).unwrap();
    let rest = tool.outline(input.clone(), &token).await.unwrap();
    assert_eq!(rest.data.symbols.len(), 2);
    assert_eq!(rest.data.symbols[0].parent, Some(1));
    assert!(!rest.data.symbols[0].signature.contains("return"));
    std::fs::write(path, source.replace("return 1", "return 3")).unwrap();
    assert!(matches!(
        tool.outline(input, &token).await,
        Err(ToolError::AccessRefused(_))
    ));
}

#[tokio::test]
async fn explicit_override_does_not_bypass_path_or_regular_text_checks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.py"), "def outside(): pass\n").unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let absolute = outside.path().join("outside.py").display().to_string();
    assert!(matches!(
        tool.outline(CodeOutlineInput::new(absolute), &token).await,
        Err(ToolError::AccessRefused(_))
    ));
    std::fs::write(root.path().join("invalid.py"), [0xff, 0xfe]).unwrap();
    assert!(matches!(
        tool.outline(CodeOutlineInput::new("invalid.py"), &token)
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    let mut directory = CodeOutlineInput::new(".");
    directory.language = Some(CodeLanguage::Python);
    assert!(matches!(
        tool.outline(directory, &token).await,
        Err(ToolError::UnsupportedResource(_))
    ));
    assert!(matches!(
        tool.outline(CodeOutlineInput::new("missing.py"), &token)
            .await,
        Err(ToolError::Io { .. })
    ));
}

#[tokio::test]
async fn sources_above_512_kib_are_refused_by_default_before_parsing() {
    let root = tempfile::tempdir().unwrap();
    let limit = CodeOutlineLimits::default().max_file_bytes;
    assert_eq!(limit, 512 * 1024);
    // One comment node: cheap to parse, so only the input bound can refuse it.
    let comment = |length: usize| format!("# {}\n", "a".repeat(length - 3));
    std::fs::write(root.path().join("large.py"), comment(limit + 1)).unwrap();
    std::fs::write(root.path().join("small.py"), comment(limit)).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let refused = tool
        .outline(CodeOutlineInput::new("large.py"), &token)
        .await;
    assert!(
        matches!(&refused, Err(ToolError::LimitExceeded(message)) if message.contains("max_file_bytes")),
        "{refused:?}"
    );
    let accepted = tool
        .outline(CodeOutlineInput::new("small.py"), &token)
        .await
        .unwrap();
    assert_eq!(accepted.data.total_symbols, 0);
}

#[tokio::test]
async fn the_outline_names_its_file_relative_to_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "fn example() {}\n").unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let absolute = root.path().join("src/lib.rs").display().to_string();
    for input in ["src/lib.rs", absolute.as_str()] {
        let page = tool
            .outline(CodeOutlineInput::new(input), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(page.data.path, "src/lib.rs");
    }
}

#[tokio::test]
async fn outside_paths_are_refused_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("present.py"), "def outside(): pass\n").unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    for path in [
        outside.path().join("present.py").display().to_string(),
        outside.path().join("missing.py").display().to_string(),
        outside
            .path()
            .join("missing/child.py")
            .display()
            .to_string(),
        "../missing-sibling/child.py".into(),
    ] {
        let result = tool
            .outline(CodeOutlineInput::new(path.clone()), &token)
            .await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_must_resolve_to_regular_files_inside_the_root() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("real.py"), "def valid(): pass\n").unwrap();
    std::fs::write(outside.path().join("private.py"), "def hidden(): pass\n").unwrap();
    symlink("real.py", root.path().join("inside.py")).unwrap();
    symlink(
        outside.path().join("private.py"),
        root.path().join("escape.py"),
    )
    .unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let result = tool
        .outline(CodeOutlineInput::new("inside.py"), &token)
        .await
        .unwrap();
    assert_eq!(result.data.symbols[0].name, "valid");
    assert!(matches!(
        tool.outline(CodeOutlineInput::new("escape.py"), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
}

/// Beneath a workspace symlink to an outside directory, a missing file is
/// refused like a present one, so the error does not reveal what exists.
#[cfg(unix)]
#[tokio::test]
async fn paths_beneath_a_symlink_to_outside_are_refused_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("present.py"), "def hidden(): pass\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    for path in [
        "link/present.py",
        "link/missing.py",
        "link/missing/child.py",
    ] {
        let result = tool.outline(CodeOutlineInput::new(path), &token).await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
    }
}

#[tokio::test]
async fn invalid_limits_and_cancellation_are_explicit_and_reusable() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("valid.rs"), "fn example() {}\n").unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let mut limits = CodeOutlineLimits::default();
    limits.max_file_bytes = 0;
    assert!(matches!(
        tool.clone().with_limits(limits),
        Err(ToolError::InvalidInput(_))
    ));
    limits = CodeOutlineLimits::default();
    limits.default_timeout_ms = limits.max_timeout_ms + 1;
    assert!(matches!(
        tool.clone().with_limits(limits),
        Err(ToolError::InvalidInput(_))
    ));
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        tool.outline(CodeOutlineInput::new("valid.rs"), &token)
            .await,
        Err(ToolError::Cancelled)
    ));
    let output = tool
        .outline(CodeOutlineInput::new("valid.rs"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.data.symbols.len(), 1);
}
