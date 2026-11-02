//! Read behavior at filesystem, cancellation and JSON boundaries.

use std::num::NonZeroUsize;

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{ReadInput, ReadLimits, ReadTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "test fixture setup must succeed")]
async fn fixture(bytes: &[u8]) -> (tempfile::TempDir, ReadTool) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("source.txt"), bytes).unwrap();
    let tool = ReadTool::new(directory.path()).await.unwrap();
    (directory, tool)
}

#[tokio::test]
async fn a_page_preserves_unicode_and_resumes_at_the_next_line() {
    let (_directory, tool) = fixture("first π\nsecond π\nthird π\n".as_bytes()).await;
    let mut input = ReadInput::new("source.txt");
    input.offset = NonZeroUsize::new(2).unwrap();
    input.limit = Some(NonZeroUsize::MIN);
    let output = tool
        .read(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.text, "2: second π\n");
    assert_eq!(output.data.lines[0].text, "second π");
    assert!(output.data.truncated);
    assert_eq!(output.data.next_offset, Some(3));

    input.offset = NonZeroUsize::new(3).unwrap();
    let last = tool.read(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(last.text, "3: third π\n");
    assert!(!last.data.truncated);
    assert_eq!(last.data.next_offset, None);
}

#[tokio::test]
async fn byte_budgets_count_utf8_and_number_prefixes() {
    let (_directory, tool) = fixture("π\nx\n".as_bytes()).await;
    let limits = ReadLimits {
        max_output_bytes: 6,
        ..ReadLimits::default()
    };
    let tool = tool.with_limits(limits).unwrap();
    let output = tool
        .read(ReadInput::new("source.txt"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.text, "1: π\n");
    assert_eq!(output.text.len(), 6);
    assert_eq!(output.data.next_offset, Some(2));
}

#[tokio::test]
async fn the_host_caps_a_models_requested_line_count() {
    let (_directory, tool) = fixture(b"one\ntwo\nthree\n").await;
    let limits = ReadLimits {
        max_lines: 1,
        ..ReadLimits::default()
    };
    let tool = tool.with_limits(limits).unwrap();
    let mut input = ReadInput::new("source.txt");
    input.limit = NonZeroUsize::new(100);
    let output = tool.read(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(output.data.lines.len(), 1);
    assert_eq!(output.data.next_offset, Some(2));
}

#[tokio::test]
async fn an_empty_file_has_an_empty_first_page() {
    let (_directory, tool) = fixture(b"").await;
    let output = tool
        .read(ReadInput::new("source.txt"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.data.lines, []);
    assert_eq!(output.text, "");
    assert!(!output.data.truncated);
}

#[tokio::test]
async fn bom_and_crlf_are_removed_from_the_view_without_changing_the_file() {
    let bytes = b"\xef\xbb\xbfhello\r\n\r\nlast";
    let (directory, tool) = fixture(bytes).await;
    let output = tool
        .read(ReadInput::new("source.txt"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.text, "1: hello\n2: \n3: last\n");
    assert_eq!(
        std::fs::read(directory.path().join("source.txt")).unwrap(),
        bytes
    );
}

#[tokio::test]
async fn a_page_past_eof_is_invalid() {
    let (_directory, tool) = fixture(b"one\n").await;
    let mut input = ReadInput::new("source.txt");
    input.offset = NonZeroUsize::new(2).unwrap();
    assert!(matches!(
        tool.read(input, &CancellationToken::new()).await,
        Err(ToolError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn binary_and_non_utf8_files_are_refused() {
    for bytes in [b"binary\0data".as_slice(), b"\xff\xfe".as_slice()] {
        let (_directory, tool) = fixture(bytes).await;
        assert!(matches!(
            tool.read(ReadInput::new("source.txt"), &CancellationToken::new())
                .await,
            Err(ToolError::UnsupportedResource(_))
        ));
    }
}

#[tokio::test]
async fn a_file_above_the_ingestion_budget_is_refused() {
    let (_directory, tool) = fixture(b"large").await;
    let limits = ReadLimits {
        max_file_bytes: 4,
        ..ReadLimits::default()
    };
    let tool = tool.with_limits(limits).unwrap();
    assert!(matches!(
        tool.read(ReadInput::new("source.txt"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
}

#[tokio::test]
async fn large_files_and_huge_unrequested_lines_do_not_expand_the_page() {
    use std::io::{Read, Write};

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.txt");
    let mut file = std::fs::File::create(&path).unwrap();
    // Generate long surrounding lines without allocating the fixture in memory.
    std::io::copy(&mut std::io::repeat(b'x').take(12 * 1024 * 1024), &mut file).unwrap();
    file.write_all("\nselected π\r\n".as_bytes()).unwrap();
    std::io::copy(&mut std::io::repeat(b'y').take(12 * 1024 * 1024), &mut file).unwrap();
    drop(file);
    let limits = ReadLimits {
        max_file_bytes: 32 * 1024 * 1024,
        max_output_bytes: 32,
        ..ReadLimits::default()
    };
    let tool = ReadTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = ReadInput::new("large.txt");
    input.offset = NonZeroUsize::new(2).unwrap();
    input.limit = NonZeroUsize::new(1);
    let output = tool.read(input, &CancellationToken::new()).await.unwrap();
    assert_eq!(output.text, "2: selected π\n");
    assert_eq!(output.data.next_offset, Some(3));
    assert_eq!(output.data.lines.len(), 1);
}

#[tokio::test]
async fn a_line_that_cannot_fit_is_reported_instead_of_silently_lost() {
    let (_directory, tool) = fixture(b"long line\n").await;
    let limits = ReadLimits {
        max_output_bytes: 4,
        ..ReadLimits::default()
    };
    let tool = tool.with_limits(limits).unwrap();
    assert!(matches!(
        tool.read(ReadInput::new("source.txt"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
}

#[tokio::test]
async fn cancellation_takes_precedence_over_starting_a_read() {
    let (_directory, tool) = fixture(b"hello").await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        tool.read(ReadInput::new("missing.txt"), &cancel).await,
        Err(ToolError::Cancelled)
    ));
}

#[tokio::test]
async fn directories_and_missing_files_have_distinct_failures() {
    let (_directory, tool) = fixture(b"hello").await;
    assert!(matches!(
        tool.read(ReadInput::new("."), &CancellationToken::new())
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    let error = tool
        .read(ReadInput::new("missing.txt"), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ToolError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
    );
}

#[tokio::test]
async fn absolute_paths_inside_the_root_work_and_paths_outside_are_refused() {
    let (directory, tool) = fixture(b"hello").await;
    let path = directory.path().join("source.txt");
    let output = tool
        .read(
            ReadInput::new(path.to_str().unwrap()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(output.text, "1: hello\n");
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(matches!(
        tool.read(
            ReadInput::new(outside.path().to_str().unwrap()),
            &CancellationToken::new()
        )
        .await,
        Err(ToolError::AccessRefused(_))
    ));
}

#[tokio::test]
async fn parent_traversal_cannot_leave_the_root() {
    let (directory, _tool) = fixture(b"private").await;
    let child = directory.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let tool = ReadTool::new(child).await.unwrap();
    assert!(matches!(
        tool.read(ReadInput::new("../source.txt"), &CancellationToken::new())
            .await,
        Err(ToolError::AccessRefused(_))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_checked_against_their_resolved_target() {
    let (directory, tool) = fixture(b"hello").await;
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join("outside")).unwrap();
    std::os::unix::fs::symlink("source.txt", directory.path().join("inside")).unwrap();
    assert!(matches!(
        tool.read(ReadInput::new("outside"), &CancellationToken::new())
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(
        tool.read(ReadInput::new("inside"), &CancellationToken::new())
            .await
            .unwrap()
            .text,
        "1: hello\n"
    );
}

#[tokio::test]
async fn concurrent_tools_keep_their_own_roots() {
    let (_first_directory, first) = fixture(b"first").await;
    let (_second_directory, second) = fixture(b"second").await;
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(
        first.read(ReadInput::new("source.txt"), &cancel),
        second.read(ReadInput::new("source.txt"), &cancel),
    );
    assert_eq!(a.unwrap().text, "1: first\n");
    assert_eq!(b.unwrap().text, "1: second\n");
}

#[test]
fn json_arguments_reject_zero_offsets_zero_limits_and_unknown_fields() {
    for invalid in [
        r#"{"path":"source.txt","offset":0}"#,
        r#"{"path":"source.txt","limit":0}"#,
        r#"{"path":"source.txt","offset":-1}"#,
        r#"{"path":"source.txt","command":"ignored?"}"#,
    ] {
        assert!(serde_json::from_str::<ReadInput>(invalid).is_err());
    }
    let input: ReadInput = serde_json::from_str(r#"{"path":"source.txt"}"#).unwrap();
    assert_eq!(input.offset.get(), 1);
    assert_eq!(input.limit, None);
}

#[tokio::test]
async fn structured_results_can_cross_a_json_boundary_without_losing_pagination() {
    let (_directory, tool) = fixture(b"hello\nworld").await;
    let mut input = ReadInput::new("source.txt");
    input.limit = Some(NonZeroUsize::MIN);
    let output = tool.read(input, &CancellationToken::new()).await.unwrap();
    let wire = serde_json::to_value(&output).unwrap();
    assert_eq!(wire["data"]["next_offset"], 2);
    assert_eq!(wire["data"]["lines"][0]["text"], "hello");
    assert_eq!(wire["text"], "1: hello\n");
    let definition = serde_json::to_value(ReadTool::definition()).unwrap();
    assert_eq!(
        definition["input_schema"]["properties"]["offset"]["minimum"],
        1
    );
    assert_eq!(definition["input_schema"]["additionalProperties"], false);
}
