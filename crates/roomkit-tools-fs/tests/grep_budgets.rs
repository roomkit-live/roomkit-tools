//! Grep's byte bounds skip what they cannot read instead of failing the
//! search.

use roomkit_tools_fs::{GrepInput, GrepLimits, GrepTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn workspace(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        let path = root.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    root
}

#[expect(clippy::unwrap_used, reason = "the tool must accept these bounds")]
async fn grep(
    root: &tempfile::TempDir,
    limits: GrepLimits,
    input: GrepInput,
) -> roomkit_tools_core::Output<roomkit_tools_fs::GrepPage> {
    GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
        .grep(input, &CancellationToken::new())
        .await
        .unwrap()
}

fn paths(page: &roomkit_tools_fs::GrepPage) -> Vec<&str> {
    page.matches
        .iter()
        .map(|found| found.path.as_str())
        .collect()
}

/// A file over `max_file_bytes` is skipped, counted and named; the other
/// files are still searched.
#[tokio::test]
async fn a_file_over_max_file_bytes_is_skipped_and_named() {
    let large = format!("hit {}\n", "x".repeat(64));
    let root = workspace(&[
        ("a.txt", b"hit\n"),
        ("big.txt", large.as_bytes()),
        ("z.txt", b"hit\n"),
    ]);
    let limits = GrepLimits {
        max_file_bytes: 16,
        ..GrepLimits::default()
    };
    let page = grep(&root, limits, GrepInput::new("hit")).await;
    assert_eq!(paths(&page.data), ["a.txt", "z.txt"]);
    assert_eq!(page.data.skipped_files, 1);
    assert!(
        page.text
            .ends_with("[skipped_files=1, first_skipped_file=\"big.txt\"]\n"),
        "{}",
        page.text
    );
}

/// Binary content is told from a file's first 8 KiB: the rest is neither
/// read nor charged to `max_total_bytes`.
#[tokio::test]
async fn a_binary_file_is_known_by_its_head_and_its_rest_is_not_charged() {
    let mut binary = b"%PDF\0".to_vec();
    binary.resize(64 * 1024, 0x80);
    let root = workspace(&[("a.pdf", &binary), ("b.txt", b"hit\n")]);
    let limits = GrepLimits {
        max_total_bytes: 16 * 1024,
        ..GrepLimits::default()
    };
    let page = grep(&root, limits, GrepInput::new("hit")).await;
    assert_eq!(paths(&page.data), ["b.txt"]);
    assert_eq!(page.data.skipped_files, 1);
    assert!(!page.data.truncated);
    assert!(
        page.text
            .ends_with("[skipped_files=1, first_skipped_file=\"a.pdf\"]\n"),
        "{}",
        page.text
    );
}

/// When `max_total_bytes` runs out, grep returns the matches found so far,
/// truncated, and names the file where it stopped, instead of failing.
#[tokio::test]
async fn the_byte_budget_returns_the_matches_found_and_names_where_it_stopped() {
    let root = workspace(&[
        ("a.txt", b"hit\n"),
        ("b.txt", b"hit\n"),
        ("c.txt", b"hit\n"),
    ]);
    let limits = GrepLimits {
        max_total_bytes: 9,
        ..GrepLimits::default()
    };
    let page = grep(&root, limits, GrepInput::new("hit")).await;
    assert_eq!(paths(&page.data), ["a.txt", "b.txt"]);
    assert!(page.data.truncated);
    assert_eq!(page.data.next_offset, None);
    assert!(
        page.text
            .ends_with("[truncated_by=max_total_bytes, stopped_at=\"c.txt\"]\n"),
        "{}",
        page.text
    );
    // A page or a pattern the budget does not reach says so, without failing.
    let mut deep = GrepInput::new("hit");
    deep.offset = std::num::NonZeroUsize::new(3).unwrap();
    for input in [deep, GrepInput::new("absent")] {
        let page = grep(&root, limits, input).await;
        assert_eq!(page.data.matches, []);
        assert!(page.data.truncated);
        assert_eq!(
            page.text,
            "no matches\n[truncated_by=max_total_bytes, stopped_at=\"c.txt\"]\n"
        );
    }
}
