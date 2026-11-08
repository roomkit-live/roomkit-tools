//! What a model reads of a file tool's result: its text alone must do.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

use std::num::NonZeroUsize;

use roomkit_tools_fs::{
    DiffInput, DiffTool, EditInput, EditTool, GlobInput, GlobTool, GrepInput, GrepLimits, GrepTool,
    ListInput, ListLimits, ListTool, PatchChange, PatchInput, PatchTool, ReadInput, ReadTool,
    WriteInput, WriteTool,
};
use tokio_util::sync::CancellationToken;

fn workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, body) in files {
        std::fs::write(root.path().join(name), body).unwrap();
    }
    root
}

#[tokio::test]
async fn a_page_ends_with_where_to_continue_and_the_revision_to_edit_against() {
    let root = workspace(&[("notes", "first\nsecond\n")]);
    let tool = ReadTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut input = ReadInput::new("notes");
    input.limit = Some(NonZeroUsize::MIN);
    let page = tool.read(input, &cancel).await.unwrap();
    assert_eq!(
        page.text,
        format!(
            "1: first\n[next_offset=2, revision={}]\n",
            page.data.revision
        )
    );
    let mut rest = ReadInput::new("notes");
    rest.offset = NonZeroUsize::new(2).unwrap();
    let last = tool.read(rest, &cancel).await.unwrap();
    assert_eq!(
        last.text,
        format!("2: second\n[revision={}]\n", last.data.revision)
    );
}

#[tokio::test]
async fn an_empty_file_still_gives_its_revision() {
    let root = workspace(&[("empty", "")]);
    let tool = ReadTool::new(root.path()).await.unwrap();
    let page = tool
        .read(ReadInput::new("empty"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(page.text, format!("[revision={}]\n", page.data.revision));
}

#[tokio::test]
async fn write_and_edit_give_the_revision_a_next_change_needs() {
    let root = workspace(&[]);
    let cancel = CancellationToken::new();
    let written = WriteTool::new(root.path())
        .await
        .unwrap()
        .write(WriteInput::new("a/notes", "hello\n"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        written.text,
        format!(
            "write \"a/notes\": created=true, changed=true, bytes=6, created_directories=[\"a\"], revision={}\n",
            written.data.revision
        )
    );
    let again = WriteTool::new(root.path())
        .await
        .unwrap()
        .write(WriteInput::new("other", "x"), &cancel)
        .await
        .unwrap();
    assert!(
        !again.text.contains("created_directories"),
        "{}",
        again.text
    );
    let edited = EditTool::new(root.path())
        .await
        .unwrap()
        .edit(EditInput::new("a/notes", "hello", "bye"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        edited.text,
        format!(
            "edit \"a/notes\": replacements=1, changed=true, bytes_before=6, bytes_after=4, revision={}\n",
            edited.data.revision
        )
    );
}

#[tokio::test]
async fn a_listing_says_where_to_continue_only_when_there_is_more() {
    let root = workspace(&[("a", ""), ("b", "")]);
    let limits = ListLimits {
        max_entries: 1,
        ..ListLimits::default()
    };
    let tool = ListTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let cancel = CancellationToken::new();
    let first = tool.list(ListInput::default(), &cancel).await.unwrap();
    assert_eq!(first.text, "1: file \"a\"\n[next_offset=2]\n");
    let whole = ListTool::new(root.path())
        .await
        .unwrap()
        .list(ListInput::default(), &cancel)
        .await
        .unwrap();
    assert_eq!(whole.text, "1: file \"a\"\n2: file \"b\"\n");
}

#[tokio::test]
async fn a_search_says_where_to_continue() {
    let root = workspace(&[("a", "hit one\nhit two\n")]);
    let limits = GrepLimits {
        max_matches: 1,
        ..GrepLimits::default()
    };
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let first = tool
        .grep(GrepInput::new("hit"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.text, "\"a\":1: hit one\n[next_offset=2]\n");
}

#[tokio::test]
async fn a_search_that_finds_nothing_says_so() {
    let root = workspace(&[("a", "text\n")]);
    let cancel = CancellationToken::new();
    let grep = GrepTool::new(root.path()).await.unwrap();
    let none = grep.grep(GrepInput::new("absent"), &cancel).await.unwrap();
    assert_eq!(none.text, "no matches\n");
    let glob = GlobTool::new(root.path()).await.unwrap();
    let none = glob
        .glob(GlobInput::new("*.absent"), &cancel)
        .await
        .unwrap();
    assert_eq!(none.text, "no entries\n");
    let empty = tempfile::tempdir().unwrap();
    let list = ListTool::new(empty.path()).await.unwrap();
    let none = list.list(ListInput::default(), &cancel).await.unwrap();
    assert_eq!(none.text, "no entries\n");
}

#[tokio::test]
async fn a_clipped_match_and_skipped_files_are_said() {
    let root = workspace(&[
        ("a", &format!("hit {}\n", "x".repeat(200))),
        ("b", "hit\0binary"),
    ]);
    let limits = GrepLimits {
        max_output_bytes: 32,
        ..GrepLimits::default()
    };
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let page = tool
        .grep(GrepInput::new("hit"), &CancellationToken::new())
        .await
        .unwrap();
    assert!(page.data.matches[0].clipped);
    let facts = page.text.lines().last().unwrap();
    assert!(facts.contains("clipped=true"), "{}", page.text);
    assert!(facts.contains("skipped_files=1"), "{}", page.text);
}

#[tokio::test]
async fn identical_files_read_as_no_differences() {
    let root = workspace(&[("a", "same\n"), ("b", "same\n")]);
    let diff = DiffTool::new(root.path())
        .await
        .unwrap()
        .diff(DiffInput::new("a", "b"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(diff.text, "no differences\n");
}

#[tokio::test]
async fn a_patch_reads_as_what_it_did() {
    let root = workspace(&[("old", "gone\n")]);
    let mut input = PatchInput::new(vec![
        PatchChange::Add {
            path: "new".into(),
            content: "x\n".into(),
        },
        PatchChange::Delete {
            path: "old".into(),
            old_content: "gone\n".into(),
        },
    ]);
    input.dry_run = false;
    let patched = PatchTool::new(root.path())
        .await
        .unwrap()
        .patch(input, &CancellationToken::new())
        .await
        .unwrap();
    let text = &patched.text;
    assert!(
        text.starts_with(
            "patch: dry_run=false, completed=true\napplied #1 add \"new\"\napplied #2 delete \"old\"\nPrepared previews (not proof of application):\n1: add \"new\"\n"
        ),
        "{text}"
    );
    assert!(text.contains("\n2: delete \"old\"\n"), "{text}");
    assert!(!text.contains("None") && !text.contains("Some("), "{text}");
}
