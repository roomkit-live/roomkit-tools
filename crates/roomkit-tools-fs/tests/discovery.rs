//! Observable filesystem discovery and text-search boundaries.

use std::num::NonZeroUsize;

use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits, GrepTool, ListInput, ListLimits,
    ListTool, PathKind,
};
use tokio_util::sync::CancellationToken;

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
        .list(ListInput::new("."), &CancellationToken::new())
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
    let mut limits = ListLimits::default();
    limits.max_entries = 1;
    let tool = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = ListInput::new("menus");
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
    let mut input = ListInput::new("empty");
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
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        tool.list(
            ListInput::new("opening-hours.txt"),
            &CancellationToken::new()
        )
        .await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        tool.list(ListInput::new("missing"), &CancellationToken::new())
            .await,
        Err(Error::Io { .. })
    ));
}

#[tokio::test]
async fn names_are_escaped_in_rendering_and_utf8_bytes_count_toward_the_budget() {
    let root = tempfile::tempdir().unwrap();
    for file in ["a", "π"] {
        std::fs::write(root.path().join(file), "").unwrap();
    }
    let mut limits = ListLimits::default();
    limits.max_output_bytes = "1: file \"a\"\n".len();
    let tool = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let first = tool
        .list(ListInput::new("."), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.text.len(), limits.max_output_bytes);
    assert_eq!(first.data.next_offset, Some(2));
    let mut input = ListInput::new(".");
    input.offset = NonZeroUsize::new(2).unwrap();
    assert!(matches!(
        tool.list(input, &CancellationToken::new()).await,
        Err(Error::LimitExceeded(_))
    ));
    #[cfg(unix)]
    {
        std::fs::write(root.path().join("line\nbreak"), "").unwrap();
        let tool = ListTool::new(root.path()).await.unwrap();
        let output = tool
            .list(ListInput::new("."), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output.text.lines().count(), 3);
        assert!(output.text.contains(r"line\nbreak"));
    }
}

#[tokio::test]
async fn scan_limit_fails_instead_of_returning_an_incomplete_inventory() {
    let root = fixture();
    let mut list_limits = ListLimits::default();
    list_limits.max_scan_entries = 2;
    let list = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(list_limits)
        .unwrap();
    assert!(matches!(
        list.list(ListInput::new("."), &CancellationToken::new())
            .await,
        Err(Error::LimitExceeded(_))
    ));
    let mut glob_limits = GlobLimits::default();
    glob_limits.max_scan_entries = 5;
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(glob_limits)
        .unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new("**"), &CancellationToken::new())
            .await,
        Err(Error::LimitExceeded(_))
    ));
    glob_limits.max_scan_entries = 100;
    glob_limits.max_depth = 1;
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(glob_limits)
        .unwrap();
    assert!(matches!(
        glob.glob(GlobInput::new("*.txt"), &CancellationToken::new())
            .await,
        Err(Error::LimitExceeded(_))
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
        Err(Error::InvalidInput(_))
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
        Err(Error::InvalidInput(_))
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
        Err(Error::InvalidInput(_))
    ));
    let grep = GrepTool::new(root.path()).await.unwrap();
    let mut input = GrepInput::new("Sourdough");
    input.include = Some(pattern);
    assert!(matches!(
        grep.grep(input, &CancellationToken::new()).await,
        Err(Error::InvalidInput(_))
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
        Err(Error::InvalidInput(_))
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
        let mut limits = GrepLimits::default();
        limits.max_file_bytes = file_limit;
        limits.max_total_bytes = total_limit;
        let tool = GrepTool::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap();
        assert!(matches!(
            tool.grep(GrepInput::new("hit"), &CancellationToken::new())
                .await,
            Err(Error::LimitExceeded(_))
        ));
    }
    let mut limits = GrepLimits::default();
    limits.max_matches = 1;
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
        Err(Error::LimitExceeded(_))
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
        list.list(ListInput::new(path), &cancel).await,
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        glob.glob(glob_input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        grep.grep(grep_input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        list.list(ListInput::new(".."), &cancel).await,
        Err(Error::Refused(_))
    ));
    cancel.cancel();
    assert!(matches!(
        list.list(ListInput::new(path), &cancel).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        glob.glob(glob_input, &cancel).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        grep.grep(grep_input, &cancel).await,
        Err(Error::Cancelled)
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
        list.list(ListInput::new("outside"), &CancellationToken::new())
            .await,
        Err(Error::Refused(_))
    ));
    assert_eq!(
        list.list(ListInput::new("inside"), &CancellationToken::new())
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
async fn non_utf8_names_fail_instead_of_returning_unusable_paths() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(std::ffi::OsString::from_vec(vec![0xff])),
        "",
    )
    .unwrap();
    let tool = ListTool::new(root.path()).await.unwrap();
    assert!(matches!(
        tool.list(ListInput::new("."), &CancellationToken::new())
            .await,
        Err(Error::Unsupported(_))
    ));
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
