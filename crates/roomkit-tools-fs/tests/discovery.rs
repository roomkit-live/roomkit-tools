//! Observable filesystem discovery and text-search boundaries.

mod support;

use std::num::NonZeroUsize;
use support::content;

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{
    GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits, GrepTool, ListInput, ListLimits,
    ListTool, PathKind,
};
use tokio_util::sync::CancellationToken;

/// A listing of `path`, Python's `list(path)`: the other arguments default.
fn listing(path: impl Into<String>) -> ListInput {
    ListInput {
        path: path.into(),
        ..ListInput::default()
    }
}

#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for directory in ["menus", "empty"] {
        std::fs::create_dir(root.path().join(directory)).unwrap();
    }
    for (file, text) in [
        (
            "opening-hours.txt",
            "Monday: closed\r\nSaturday: 08:30-16:00\r\n",
        ),
        ("menus/bread.txt", "Sourdough: 5.50\nContains gluten\n"),
        ("orders.json", "{\"order\":42}\n"),
        (".hidden.txt", "Hidden note\n"),
        ("menus/logπ.txt", "logπ\nContains MILK\n"),
    ] {
        std::fs::write(root.path().join(file), text).unwrap();
    }
    root
}

#[tokio::test]
async fn list_is_sorted_non_recursive_and_includes_hidden_entries() {
    let root = fixture();
    let tool = ListTool::new(root.path()).await.unwrap();
    let output = tool
        .list(ListInput::default(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        output
            .data
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        [
            ".hidden.txt",
            "empty",
            "menus",
            "opening-hours.txt",
            "orders.json"
        ]
    );
    assert_eq!(output.data.entries[2].kind, PathKind::Directory);
    assert_eq!(output.data.entries[3].kind, PathKind::File);
    assert!(!output.data.truncated);
}

#[tokio::test]
async fn list_pagination_uses_host_limits_and_workspace_relative_paths() {
    let root = fixture();
    let limits = ListLimits {
        max_entries: 1,
        ..ListLimits::default()
    };
    let tool = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = listing("menus");
    input.limit = NonZeroUsize::new(100);
    let first = tool
        .list(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.data.entries[0].path, "menus/bread.txt");
    assert_eq!(first.data.next_offset, Some(2));
    input.offset = NonZeroUsize::new(2).unwrap();
    let last = tool.list(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(last.data.entries[0].path, "menus/logπ.txt");
    assert_eq!(last.data.next_offset, None);
}

#[tokio::test]
async fn empty_directory_and_past_end_offset_are_distinct() {
    let root = fixture();
    let tool = ListTool::new(root.path()).await.unwrap();
    let mut input = listing("empty");
    assert_eq!(
        tool.list(input.clone(), &CancellationToken::new())
            .await
            .unwrap()
            .data
            .entries,
        []
    );
    input.offset = NonZeroUsize::new(2).unwrap();
    assert!(matches!(
        tool.list(input, &CancellationToken::new()).await,
        Err(ToolError::InvalidInput(_))
    ));
    assert!(matches!(
        tool.list(listing("opening-hours.txt"), &CancellationToken::new())
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    assert!(matches!(
        tool.list(listing("missing"), &CancellationToken::new())
            .await,
        Err(ToolError::InvalidInput(message)) if message == "no such file or directory: missing"
    ));
}

#[tokio::test]
async fn names_are_escaped_in_rendering_and_utf8_bytes_count_toward_the_budget() {
    let root = tempfile::tempdir().unwrap();
    for file in ["a", "π"] {
        std::fs::write(root.path().join(file), "").unwrap();
    }
    let limits = ListLimits {
        max_output_bytes: "1: file \"a\"\n".len(),
        ..ListLimits::default()
    };
    let tool = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let first = tool
        .list(ListInput::default(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(content(&first.text).len(), limits.max_output_bytes);
    assert_eq!(first.data.next_offset, Some(2));
    let input = ListInput {
        offset: NonZeroUsize::new(2).unwrap(),
        ..ListInput::default()
    };
    assert!(matches!(
        tool.list(input, &CancellationToken::new()).await,
        Err(ToolError::LimitExceeded(_))
    ));
    #[cfg(unix)]
    {
        std::fs::write(root.path().join("line\nbreak"), "").unwrap();
        let tool = ListTool::new(root.path()).await.unwrap();
        let output = tool
            .list(ListInput::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output.text.lines().count(), 3);
        assert!(output.text.contains(r"line\nbreak"));
    }
}

#[tokio::test]
async fn scan_limit_fails_instead_of_returning_an_incomplete_inventory() {
    let root = fixture();
    let list_limits = ListLimits {
        max_scan_entries: 2,
        ..ListLimits::default()
    };
    let list = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(list_limits)
        .unwrap();
    assert!(matches!(
        list.list(ListInput::default(), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    let mut glob_limits = GlobLimits {
        max_scan_entries: 5,
        ..GlobLimits::default()
    };
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(glob_limits)
        .unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new("**"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    glob_limits.max_scan_entries = 100;
    glob_limits.max_depth = 1;
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(glob_limits)
        .unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new("**/*.txt"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
}

#[tokio::test]
async fn glob_patterns_are_relative_to_the_search_directory_and_pages_resume() {
    let root = fixture();
    let tool = GlobTool::new(root.path()).await.unwrap();
    let mut input = GlobInput::new("*.txt");
    input.path = "menus".into();
    input.limit = Some(NonZeroUsize::MIN);
    let first = tool
        .glob(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.data.entries[0].path, "menus/bread.txt");
    input.offset = NonZeroUsize::new(first.data.next_offset.unwrap()).unwrap();
    let last = tool.glob(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(last.data.entries[0].path, "menus/logπ.txt");
    assert_eq!(last.data.next_offset, None);
    let direct = tool
        .glob(GlobInput::new("*.txt"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(direct.data.entries.len(), 2);
    let recursive = tool
        .glob(GlobInput::new("**/*.txt"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(recursive.data.entries.len(), 4);
    let directories = tool
        .glob(GlobInput::new("menus"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(directories.data.entries[0].kind, PathKind::Directory);
}

#[tokio::test]
async fn invalid_patterns_and_no_matches_are_distinct() {
    let root = fixture();
    let glob = GlobTool::new(root.path()).await.unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new("["), &CancellationToken::new())
            .await,
        Err(ToolError::InvalidInput(_))
    ));
    assert_eq!(
        glob.glob(GlobInput::new("*.missing"), &CancellationToken::new())
            .await
            .unwrap()
            .data
            .entries,
        []
    );
    let grep = GrepTool::new(root.path()).await.unwrap();
    assert!(matches!(
        grep.grep(GrepInput::new("["), &CancellationToken::new())
            .await,
        Err(ToolError::InvalidInput(_))
    ));
    assert_eq!(
        grep.grep(
            GrepInput::new("not present anywhere"),
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .data
        .matches,
        []
    );
}

#[tokio::test]
async fn deeply_nested_brace_globs_are_invalid_input_instead_of_a_panic() {
    let root = fixture();
    let pattern = format!("{}a{}", "{".repeat(300), "}".repeat(300));
    assert!(pattern.len() <= 4096);
    let glob = GlobTool::new(root.path()).await.unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new(pattern.clone()), &CancellationToken::new())
            .await,
        Err(ToolError::InvalidInput(_))
    ));
    let grep = GrepTool::new(root.path()).await.unwrap();
    let mut input = GrepInput::new("Sourdough");
    input.include = Some(pattern);
    assert!(matches!(
        grep.grep(input, &CancellationToken::new()).await,
        Err(ToolError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn grep_filters_files_and_returns_paths_and_source_line_numbers() {
    let root = fixture();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let mut input = GrepInput::new("contains (gluten|milk)");
    input.ignore_case = true;
    input.include = Some("**/*.txt".into());
    input.limit = Some(NonZeroUsize::MIN);
    let first = tool
        .grep(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.data.matches[0].path, "menus/bread.txt");
    assert_eq!(first.data.matches[0].line, 2);
    assert_eq!(first.data.matches[0].text, "Contains gluten");
    assert_eq!(first.data.next_offset, Some(2));
    input.offset = NonZeroUsize::new(2).unwrap();
    let last = tool
        .grep(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(last.data.matches[0].path, "menus/logπ.txt");
    assert_eq!(last.data.next_offset, None);
    input.offset = NonZeroUsize::new(3).unwrap();
    assert!(matches!(
        tool.grep(input, &CancellationToken::new()).await,
        Err(ToolError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn literal_search_treats_regex_characters_as_text() {
    let root = fixture();
    std::fs::write(root.path().join("literal.txt"), "a.b\naxb\n").unwrap();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let mut input = GrepInput::new("a.b");
    input.path = "literal.txt".into();
    input.literal = true;
    let literal = tool
        .grep(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(literal.data.matches.len(), 1);
    input.literal = false;
    assert_eq!(
        tool.grep(input, &CancellationToken::new())
            .await
            .unwrap()
            .data
            .matches
            .len(),
        2
    );
}

#[tokio::test]
async fn binary_files_are_counted_and_bom_crlf_are_handled() {
    let root = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("a.bin", b"bad\0text".as_slice()),
        ("b.bin", b"\xff".as_slice()),
        ("c.txt", b"\xef\xbb\xbfMonday\r\nSaturday\r\n".as_slice()),
    ] {
        std::fs::write(root.path().join(name), bytes).unwrap();
    }
    let tool = GrepTool::new(root.path()).await.unwrap();
    let output = tool
        .grep(GrepInput::new("Saturday"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.data.skipped_files, 2);
    assert_eq!(output.data.matches[0].line, 2);
    assert_eq!(output.data.matches[0].text, "Saturday");
}

#[tokio::test]
async fn grep_caps_file_bytes_total_bytes_matches_and_rendered_bytes() {
    let root = tempfile::tempdir().unwrap();
    for file in ["a", "b"] {
        std::fs::write(root.path().join(file), "hit\n").unwrap();
    }
    for (file_limit, total_limit) in [(3, 100), (100, 7)] {
        let limits = GrepLimits {
            max_file_bytes: file_limit,
            max_total_bytes: total_limit,
            ..GrepLimits::default()
        };
        let tool = GrepTool::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap();
        assert!(matches!(
            tool.grep(GrepInput::new("hit"), &CancellationToken::new())
                .await,
            Err(ToolError::LimitExceeded(_))
        ));
    }
    let mut limits = GrepLimits {
        max_matches: 1,
        ..GrepLimits::default()
    };
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = GrepInput::new("hit");
    input.limit = NonZeroUsize::new(100);
    let output = tool.grep(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(output.data.matches.len(), 1);
    assert_eq!(output.data.next_offset, Some(2));
    limits.max_output_bytes = 1;
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    assert!(matches!(
        tool.grep(GrepInput::new("hit"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
}

#[tokio::test]
async fn all_discovery_tools_refuse_root_escape_and_honor_pre_cancellation() {
    let root = fixture();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().to_str().unwrap();
    let list = ListTool::new(root.path()).await.unwrap();
    let glob = GlobTool::new(root.path()).await.unwrap();
    let grep = GrepTool::new(root.path()).await.unwrap();
    let mut glob_input = GlobInput::new("**");
    glob_input.path = path.into();
    let mut grep_input = GrepInput::new(".");
    grep_input.path = path.into();
    let cancel = CancellationToken::new();
    assert!(matches!(
        list.list(listing(path), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        glob.glob(glob_input.clone(), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        grep.grep(grep_input.clone(), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        list.list(listing(".."), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    cancel.cancel();
    assert!(matches!(
        list.list(listing(path), &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(matches!(
        glob.glob(glob_input, &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(matches!(
        grep.grep(grep_input, &cancel).await,
        Err(ToolError::Cancelled)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_listed_but_never_recursed_into() {
    let root = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "PRIVATE").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("outside")).unwrap();
    std::os::unix::fs::symlink("menus", root.path().join("inside")).unwrap();
    std::os::unix::fs::symlink("missing", root.path().join("broken")).unwrap();
    let glob = GlobTool::new(root.path()).await.unwrap();
    let paths = glob
        .glob(GlobInput::new("**"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        paths
            .data
            .entries
            .iter()
            .filter(|entry| entry.kind == PathKind::Symlink)
            .count(),
        3
    );
    assert!(
        !paths
            .data
            .entries
            .iter()
            .any(|entry| entry.path.starts_with("inside/") || entry.path.starts_with("outside/"))
    );
    let list = ListTool::new(root.path()).await.unwrap();
    assert!(matches!(
        list.list(listing("outside"), &CancellationToken::new())
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(
        list.list(listing("inside"), &CancellationToken::new())
            .await
            .unwrap()
            .data
            .entries
            .len(),
        2
    );
    let grep = GrepTool::new(root.path()).await.unwrap();
    assert_eq!(
        grep.grep(GrepInput::new("PRIVATE"), &CancellationToken::new())
            .await
            .unwrap()
            .data
            .matches,
        []
    );
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_names_are_skipped_and_counted_anywhere_in_the_tree() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(OsString::from_vec(vec![0xff])), "hit\n").unwrap();
    let hidden = root.path().join("sub").join(OsString::from_vec(vec![0xfe]));
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join("inner.txt"), "hit\n").unwrap();
    std::fs::write(root.path().join("sub/ok.txt"), "hit\n").unwrap();
    let cancel = CancellationToken::new();
    let list = ListTool::new(root.path()).await.unwrap();
    let listing = list.list(ListInput::default(), &cancel).await.unwrap();
    assert_eq!(listing.data.entries.len(), 1);
    assert_eq!(listing.data.skipped_entries, 1);
    assert!(
        listing.text.ends_with("[skipped_entries=1]\n"),
        "{}",
        listing.text
    );
    let glob = GlobTool::new(root.path()).await.unwrap();
    let found = glob
        .glob(GlobInput::new("**/*.txt"), &cancel)
        .await
        .unwrap();
    assert_eq!(found.data.entries.len(), 1);
    assert_eq!(found.data.entries[0].path, "sub/ok.txt");
    assert_eq!(found.data.skipped_entries, 2);
    let grep = GrepTool::new(root.path()).await.unwrap();
    let matches = grep.grep(GrepInput::new("hit"), &cancel).await.unwrap();
    assert_eq!(matches.data.matches.len(), 1);
    assert_eq!(matches.data.matches[0].path, "sub/ok.txt");
    assert_eq!(matches.data.skipped_entries, 2);
}

#[tokio::test]
async fn patterns_that_cannot_match_deeper_do_not_scan_deeper_directories() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("top.rs"), "hit\n").unwrap();
    let mut deep = root.path().join("deep");
    for level in 0..5 {
        std::fs::create_dir(&deep).unwrap();
        for file in 0..4 {
            std::fs::write(deep.join(format!("{level}-{file}.rs")), "hit\n").unwrap();
        }
        deep.push("deep");
    }
    let cancel = CancellationToken::new();
    let glob_limits = GlobLimits {
        max_scan_entries: 8,
        max_depth: 2,
        ..GlobLimits::default()
    };
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(glob_limits)
        .unwrap();
    let shallow = glob.glob(GlobInput::new("*.rs"), &cancel).await.unwrap();
    assert_eq!(shallow.data.entries.len(), 1);
    assert_eq!(
        glob.glob(GlobInput::new("deep/*.rs"), &cancel)
            .await
            .unwrap()
            .data
            .entries
            .len(),
        4
    );
    for unbounded in ["**/*.rs", "deep/*/*.rs", "[d]eep/*.rs"] {
        assert!(
            matches!(
                glob.glob(GlobInput::new(unbounded), &cancel).await,
                Err(ToolError::LimitExceeded(_))
            ),
            "{unbounded}"
        );
    }
    let grep_limits = GrepLimits {
        max_scan_entries: 8,
        max_depth: 2,
        ..GrepLimits::default()
    };
    let grep = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(grep_limits)
        .unwrap();
    let mut input = GrepInput::new("hit");
    input.include = Some("deep/*.rs".into());
    let matches = grep.grep(input.clone(), &cancel).await.unwrap();
    assert_eq!(matches.data.matches.len(), 4);
    // Without a `/`, an include filters names at any depth, so it scans them all.
    for unbounded in ["*.rs", "**/*.rs"] {
        input.include = Some(unbounded.into());
        assert!(
            matches!(
                grep.grep(input.clone(), &cancel).await,
                Err(ToolError::LimitExceeded(_))
            ),
            "{unbounded}"
        );
    }
}

/// `include: *.rs` used to read only the search directory, and its
/// "no matches" read as an absence (RMK-1639).
#[tokio::test]
async fn an_include_without_a_slash_filters_file_names_at_any_depth() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("src/delegation");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(nested.join("mod.rs"), "fn worker_note() {}\n").unwrap();
    std::fs::write(nested.join("notes.md"), "fn worker_note, in prose\n").unwrap();
    let grep = GrepTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("fn worker_note");
    input.path = "src".into();
    for (include, expected) in [
        ("*.rs", &["src/delegation/mod.rs"][..]),
        ("*.md", &["src/delegation/notes.md"]),
        ("mod.*", &["src/delegation/mod.rs"]),
        // With a `/`, the glob matches the path below `path`, `*` within a level.
        ("delegation/*.rs", &["src/delegation/mod.rs"]),
        ("*/*", &["src/delegation/mod.rs", "src/delegation/notes.md"]),
        ("/*.rs", &[]),
        ("src/**/*.rs", &[]),
    ] {
        input.include = Some(include.into());
        let found = grep.grep(input.clone(), &cancel).await.unwrap();
        let paths: Vec<_> = found.data.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, expected, "{include}");
    }
}

#[test]
fn argument_schemas_keep_defaults_and_reject_bad_offsets_and_unknown_keys() {
    let input: ListInput = serde_json::from_str("{}").unwrap();
    assert_eq!(input.path, ".");
    assert_eq!(input.offset.get(), 1);
    for invalid in [r#"{"offset":0}"#, r#"{"limit":0}"#, r#"{"recursive":true}"#] {
        assert!(serde_json::from_str::<ListInput>(invalid).is_err());
    }
    for definition in [
        ListTool::definition(),
        GlobTool::definition(),
        GrepTool::definition(),
    ] {
        assert_eq!(definition.input_schema["additionalProperties"], false);
        assert_eq!(
            definition.input_schema["properties"]["offset"]["minimum"],
            1
        );
    }
}
