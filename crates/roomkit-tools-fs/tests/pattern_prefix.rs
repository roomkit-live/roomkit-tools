//! A glob, or a grep `include` with `/`, is searched from its literal
//! directories, so a large sibling directory does not use up the scan bound,
//! and the results are those a walk from `path` would give.

use std::path::Path;

use roomkit_tools_fs::{GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits, GrepTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `src/` with two Rust files beside a `data/` directory larger than the
/// scan bound; `.gitignore` names `target/`.
#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for file in ["src/main.rs", "src/nested/lib.rs", "target/build.rs"] {
        write(&root.path().join(file), "fn needle() {}\n");
    }
    for index in 0..20 {
        write(&root.path().join(format!("data/{index}.json")), "{}\n");
    }
    write(&root.path().join(".gitignore"), "target/\n");
    root
}

#[expect(clippy::unwrap_used, reason = "the search must succeed")]
async fn globbed(root: &Path, pattern: &str) -> (Vec<String>, bool) {
    let tool = GlobTool::new(root)
        .await
        .unwrap()
        .with_limits(GlobLimits {
            max_scan_entries: 10,
            ..GlobLimits::default()
        })
        .unwrap();
    let found = tool
        .glob(GlobInput::new(pattern), &CancellationToken::new())
        .await
        .unwrap();
    let paths = found.data.entries.into_iter().map(|e| e.path).collect();
    (paths, found.data.truncated)
}

#[tokio::test]
async fn a_glob_is_searched_from_its_literal_directories() {
    let root = project();
    assert_eq!(
        globbed(root.path(), "src/**/*.rs").await,
        (
            vec!["src/main.rs".into(), "src/nested/lib.rs".into()],
            false
        )
    );
    let grep = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(GrepLimits {
            max_scan_entries: 10,
            ..GrepLimits::default()
        })
        .unwrap();
    let mut input = GrepInput::new("needle");
    input.include = Some("src/**/*.rs".into());
    let found = grep.grep(input, &CancellationToken::new()).await.unwrap();
    let paths: Vec<_> = found.data.matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["src/main.rs", "src/nested/lib.rs"]);
    assert!(!found.data.truncated);
}

/// A literal directory the walk would not have entered gives no match:
/// missing, a file, ignored, or a symbolic link.
#[tokio::test]
async fn a_literal_directory_the_walk_would_not_enter_matches_nothing() {
    let root = project();
    #[cfg(unix)]
    std::os::unix::fs::symlink("src", root.path().join("link")).unwrap();
    for pattern in [
        "absent/**/*.rs",
        "src/main.rs/*",
        "target/**/*.rs",
        "link/*.rs",
    ] {
        assert_eq!(
            globbed(root.path(), pattern).await,
            (vec![], false),
            "{pattern}"
        );
    }
}
