//! File boundaries, revision-consistent paging and resource contracts.
use std::num::NonZeroUsize;

use roomkit_tools_code::{CodeLanguage, CodeOutlineInput, CodeOutlineLimits, CodeOutlineTool};
use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_model_reads_the_symbols_where_to_continue_and_the_revision() {
    let root = tempfile::tempdir().unwrap();
    let source = "class Service:\n    def one(self):\n        return 1\n";
    std::fs::write(root.path().join("code.py"), source).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let mut input = CodeOutlineInput::new("code.py");
    input.limit = NonZeroUsize::new(1);
    let first = tool.outline(input.clone(), &token).await.unwrap();
    assert_eq!(
        first.text,
        format!(
            "Python outline\n1 [1-3] class Service:\n[next_offset=2, revision={}]\n",
            first.data.revision
        )
    );
    input.offset = NonZeroUsize::new(2).unwrap();
    let rest = tool.outline(input, &token).await.unwrap();
    assert_eq!(
        rest.text,
        format!(
            "Python outline\n2 [2-3] ^1 def one(self):\n[revision={}]\n",
            rest.data.revision
        )
    );
}

/// One line per symbol: its index, range, parent and signature, the
/// signature's whitespace collapsed; `data` keeps the source header.
#[tokio::test]
async fn each_symbol_is_one_line_with_its_signature_collapsed() {
    let root = tempfile::tempdir().unwrap();
    let source = "impl Totals {\n    #[inline]\n    pub fn add(\n        a: u32,\n        \
        b: u32,\n    ) -> u32 {\n        a + b\n    }\n}\n";
    std::fs::write(root.path().join("lib.rs"), source).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let page = tool
        .outline(CodeOutlineInput::new("lib.rs"), &token)
        .await
        .unwrap();
    assert_eq!(
        page.text,
        format!(
            "Rust outline\n1 [1-9] impl Totals\n\
             2 [2-8] ^1 #[inline] pub fn add( a: u32, b: u32, ) -> u32\n\
             [revision={}]\n",
            page.data.revision
        )
    );
    assert_eq!(
        page.data.symbols[1].signature,
        "#[inline]\n    pub fn add(\n        a: u32,\n        b: u32,\n    ) -> u32"
    );
    let limits = CodeOutlineLimits {
        max_signature_bytes: 25,
        ..CodeOutlineLimits::default()
    };
    let clipped = tool
        .with_limits(limits)
        .unwrap()
        .outline(CodeOutlineInput::new("lib.rs"), &token)
        .await
        .unwrap();
    assert!(
        clipped
            .text
            .contains("\n2 [2-8] ^1 #[inline] pub fn add( [signature_truncated: method \"add\"]\n"),
        "{}",
        clipped.text
    );
}

/// A signature clipped before the declaration, behind a long decorator or
/// attribute, still names the symbol and its kind.
#[tokio::test]
async fn a_signature_clipped_before_its_declaration_still_names_the_symbol() {
    let root = tempfile::tempdir().unwrap();
    let rows: Vec<String> = (0..60).map(|n| format!("    ({n}, \"{n}\"),")).collect();
    let python = format!(
        "@pytest.mark.parametrize(\n    \"value, expected\",\n    [\n{}\n    ],\n)\n\
         def test_conversion(value, expected):\n    assert str(value) == expected\n",
        rows.join("\n")
    );
    let rust = format!(
        "#[doc = \"{}\"]\npub struct Documented;\n",
        "x".repeat(1100)
    );
    std::fs::write(root.path().join("test_table.py"), python).unwrap();
    std::fs::write(root.path().join("lib.rs"), rust).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    for (path, named) in [
        (
            "test_table.py",
            " [signature_truncated: function \"test_conversion\"]\n",
        ),
        ("lib.rs", " [signature_truncated: struct \"Documented\"]\n"),
    ] {
        let page = tool
            .outline(CodeOutlineInput::new(path), &token)
            .await
            .unwrap();
        assert!(page.data.symbols[0].signature_truncated, "{path}");
        let line = page.text.lines().nth(1).unwrap();
        assert!(format!("{line}\n").ends_with(named), "{path}: {line}");
    }
}

/// On one line, a line comment would swallow what follows it: the line
/// leaves the header's comments out; `data` keeps them.
#[tokio::test]
async fn a_header_s_comments_stay_out_of_its_line() {
    let root = tempfile::tempdir().unwrap();
    let rust = "pub fn add(\n    a: u32, // the first\n    b: u32, /* the second */\n) -> u32 {\n    \
        a + b\n}\n";
    let python = "def add(\n    a,  # the first\n    b,\n):\n    return a + b\n\n\
        @decorator  # between\n@other\ndef f():\n    pass\n";
    std::fs::write(root.path().join("lib.rs"), rust).unwrap();
    std::fs::write(root.path().join("code.py"), python).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let lines = |page: &roomkit_tools_core::Output<roomkit_tools_code::CodeOutlinePage>| {
        page.text
            .lines()
            .skip(1)
            .take(page.data.symbols.len())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let page = tool
        .outline(CodeOutlineInput::new("lib.rs"), &token)
        .await
        .unwrap();
    assert_eq!(
        lines(&page),
        ["1 [1-6] pub fn add( a: u32, b: u32, ) -> u32"]
    );
    assert!(page.data.symbols[0].signature.contains("// the first"));
    let page = tool
        .outline(CodeOutlineInput::new("code.py"), &token)
        .await
        .unwrap();
    assert_eq!(
        lines(&page),
        [
            "1 [1-5] def add( a, b, ):",
            "2 [7-10] @decorator @other def f():"
        ]
    );
}

/// A Rust symbol's range covers the attributes and outer doc comments
/// written before it, in any order; a plain comment is not part of it.
#[tokio::test]
async fn a_rust_range_covers_its_doc_comments_and_attributes() {
    let root = tempfile::tempdir().unwrap();
    let source = "#[derive(Debug)]\n/// A point.\npub struct Point;\n\n\
        /// Doc first.\n#[inline]\nfn first() {}\n\n\
        /** A block doc. */\nfn block() {}\n\n\
        // A plain comment.\nfn plain() {}\n";
    std::fs::write(root.path().join("lib.rs"), source).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let page = tool
        .outline(CodeOutlineInput::new("lib.rs"), &CancellationToken::new())
        .await
        .unwrap();
    let ranges: Vec<_> = page
        .data
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.start_line, s.end_line))
        .collect();
    assert_eq!(
        ranges,
        [
            ("Point", 1, 3),
            ("first", 5, 7),
            ("block", 9, 10),
            ("plain", 13, 13),
        ]
    );
    // The signature stays the header: attributes right before the item,
    // no doc comment.
    assert_eq!(page.data.symbols[0].signature, "pub struct Point;");
    assert_eq!(page.data.symbols[1].signature, "#[inline]\nfn first()");
}

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
    let limits = CodeOutlineLimits {
        max_output_bytes: serde_json::to_vec(&first).unwrap().len() + 20,
        ..CodeOutlineLimits::default()
    };
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
        Err(ToolError::InvalidInput(message)) if message == "no such file or directory: missing.py"
    ));
}

/// A model that outlines a Makefile is told to read it, not to set a
/// language; one that outlines a directory is told it is no file.
#[tokio::test]
async fn an_unsupported_file_and_a_directory_are_refused_for_what_they_are() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Makefile"), "all:\n\tcargo build\n").unwrap();
    std::fs::create_dir(root.path().join("pkg")).unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let refusal = |path: &'static str| {
        let tool = tool.clone();
        let token = token.clone();
        async move {
            match tool.outline(CodeOutlineInput::new(path), &token).await {
                Err(ToolError::UnsupportedResource(message)) => message,
                other => panic!("{path}: {other:?}"),
            }
        }
    };
    assert_eq!(
        refusal("Makefile").await,
        "code_outline reads .rs, .py and .pyi files; use read for Makefile"
    );
    assert_eq!(
        refusal("pkg").await,
        "code_outline accepts regular files only"
    );
    let schema = CodeOutlineTool::definition().input_schema;
    assert_eq!(
        schema["properties"]["language"]["description"],
        "Omit for .rs, .py and .pyi files. Set only to parse another file as Rust or Python."
    );
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
    let limits = CodeOutlineLimits {
        max_file_bytes: 0,
        ..CodeOutlineLimits::default()
    };
    assert!(matches!(
        tool.clone().with_limits(limits),
        Err(ToolError::InvalidInput(_))
    ));
    let limits = CodeOutlineLimits {
        default_timeout_ms: CodeOutlineLimits::default().max_timeout_ms + 1,
        ..CodeOutlineLimits::default()
    };
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
