//! Recursive filename patterns keep their meaning whatever strategy the glob
//! matcher picks for them (literal, extension, basename-only or full-path).

use roomkit_tools_fs::{GlobInput, GlobTool, GrepInput, GrepTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for directory in [".venv", "src/deep", "src/b.pyc.d"] {
        std::fs::create_dir_all(root.path().join(directory)).unwrap();
    }
    for file in [
        "a.pyc",
        "ab.pyc",
        ".b.pyc",
        "src/b.pyc",
        "src/b.pyx",
        "src/.venv-old",
        "src/deep/c.pyo",
        "src/deep/c.PYC",
        "src/b.pyc.d/inner.txt",
        "src/*.txt",
        "src/x.txt",
    ] {
        std::fs::write(root.path().join(file), "needle\n").unwrap();
    }
    root
}

#[expect(clippy::unwrap_used, reason = "a valid pattern must match")]
async fn glob(tool: &GlobTool, pattern: &str) -> Vec<String> {
    let output = tool
        .glob(GlobInput::new(pattern), &CancellationToken::new())
        .await
        .unwrap();
    let mut paths: Vec<_> = output.data.entries.into_iter().map(|e| e.path).collect();
    paths.sort();
    paths
}

#[tokio::test]
async fn recursive_filename_patterns_match_only_the_final_component() {
    let root = fixture();
    let tool = GlobTool::new(root.path()).await.unwrap();
    for (pattern, expected) in [
        (
            "**/*.py[cod]",
            &["a.pyc", "ab.pyc", ".b.pyc", "src/b.pyc", "src/deep/c.pyo"][..],
        ),
        ("**/.venv*", &[".venv", "src/.venv-old"]),
        ("**/[!.]*.pyc", &["a.pyc", "ab.pyc", "src/b.pyc"]),
        ("**/?.pyc", &["a.pyc", "src/b.pyc"]),
        ("**/b.py[!c]", &["src/b.pyx"]),
        (
            "**/*.pyc*",
            &["a.pyc", "ab.pyc", ".b.pyc", "src/b.pyc", "src/b.pyc.d"],
        ),
        ("src/*.py?", &["src/b.pyc", "src/b.pyx"]),
        ("**/\\*.txt", &["src/*.txt"]),
        ("**/*.PYC", &["src/deep/c.PYC"]),
    ] {
        let mut expected: Vec<_> = expected.iter().map(|path| (*path).to_owned()).collect();
        expected.sort();
        assert_eq!(glob(&tool, pattern).await, expected, "{pattern}");
    }
}

#[tokio::test]
async fn grep_includes_use_the_same_recursive_filename_semantics() {
    let root = fixture();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let mut input = GrepInput::new("needle");
    input.include = Some("**/[!.]*.py[cod]".into());
    let output = tool.grep(input, &CancellationToken::new()).await.unwrap();
    let mut files: Vec<_> = output
        .data
        .matches
        .into_iter()
        .map(|found| found.path)
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["a.pyc", "ab.pyc", "src/b.pyc", "src/deep/c.pyo"].map(String::from)
    );
}
