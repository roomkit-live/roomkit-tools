//! At a traversal bound, list, glob and grep return what they found, in path
//! order, and say which bound stopped them and where.

use roomkit_tools_fs::{
    GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits, GrepTool, ListInput, ListLimits,
    ListTool,
};
use tokio_util::sync::CancellationToken;

/// Directories `a`, `b` and `c`, each holding three files that say `hit`.
#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn three_directories() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for directory in ["a", "b", "c"] {
        std::fs::create_dir(root.path().join(directory)).unwrap();
        for file in ["1", "2", "3"] {
            std::fs::write(root.path().join(directory).join(file), "hit\n").unwrap();
        }
    }
    root
}

/// A directory with more entries than `max_scan_entries` lists those it
/// read, sorted, instead of failing.
#[tokio::test]
async fn a_directory_over_the_scan_bound_lists_the_entries_read() {
    let root = tempfile::tempdir().unwrap();
    for file in ["a", "b", "c", "d", "e"] {
        std::fs::write(root.path().join(file), "").unwrap();
    }
    let list = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(ListLimits {
            max_scan_entries: 3,
            ..ListLimits::default()
        })
        .unwrap();
    let page = list
        .list(ListInput::default(), &CancellationToken::new())
        .await
        .unwrap();
    let names: Vec<_> = page.data.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names.len(), 3);
    assert!(names.is_sorted(), "{names:?}");
    assert!(page.data.truncated);
    assert_eq!(page.data.next_offset, None);
    assert!(
        page.text
            .ends_with("[truncated_by=max_scan_entries, stopped_at=\".\"]\n"),
        "{}",
        page.text
    );
}

/// A walk visits directories in path order, so at the bound it holds the
/// first ones whole and names the one it stopped in.
#[tokio::test]
async fn a_walk_at_the_scan_bound_returns_what_it_found_in_path_order() {
    let root = three_directories();
    let cancel = CancellationToken::new();
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(GlobLimits {
            max_scan_entries: 6,
            ..GlobLimits::default()
        })
        .unwrap();
    let found = glob.glob(GlobInput::new("**"), &cancel).await.unwrap();
    let paths: Vec<_> = found.data.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["a", "a/1", "a/2", "a/3", "b", "c"]);
    assert!(found.data.truncated);
    assert!(
        found
            .text
            .ends_with("[truncated_by=max_scan_entries, stopped_at=\"b\"]\n"),
        "{}",
        found.text
    );
    let grep = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(GrepLimits {
            max_scan_entries: 6,
            ..GrepLimits::default()
        })
        .unwrap();
    let found = grep.grep(GrepInput::new("hit"), &cancel).await.unwrap();
    let paths: Vec<_> = found.data.matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["a/1", "a/2", "a/3"]);
    assert!(found.data.truncated);
    assert_eq!(found.data.next_offset, None);
    assert!(
        found
            .text
            .ends_with("[truncated_by=max_scan_entries, stopped_at=\"b\"]\n"),
        "{}",
        found.text
    );
}

/// A directory past `max_depth` is listed but not entered, and named.
#[tokio::test]
async fn a_directory_past_max_depth_is_listed_but_not_entered() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("one/two/three")).unwrap();
    std::fs::write(root.path().join("one/two/three/deep.txt"), "hit\n").unwrap();
    std::fs::write(root.path().join("one/top.txt"), "hit\n").unwrap();
    let glob = GlobTool::new(root.path())
        .await
        .unwrap()
        .with_limits(GlobLimits {
            max_depth: 2,
            ..GlobLimits::default()
        })
        .unwrap();
    let found = glob
        .glob(GlobInput::new("**"), &CancellationToken::new())
        .await
        .unwrap();
    let paths: Vec<_> = found.data.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["one", "one/top.txt", "one/two"]);
    assert!(found.data.truncated);
    assert!(
        found
            .text
            .ends_with("[truncated_by=max_depth, stopped_at=\"one/two\"]\n"),
        "{}",
        found.text
    );
}
