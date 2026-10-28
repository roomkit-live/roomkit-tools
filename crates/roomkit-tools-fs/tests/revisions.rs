//! Content revisions protect mutations without hidden per-agent read history.

use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    EditInput, EditTool, MutationWorkspace, ReadInput, ReadTool, WriteInput, WriteTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn revisions_hash_raw_bytes_and_are_independent_of_the_page() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes");
    std::fs::write(&path, "abc").unwrap();
    let read = ReadTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let first = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
    assert_eq!(
        first.data.revision,
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    for contents in ["\u{feff}abc\r\nsecond\r\n", "abc\nsecond\n"] {
        std::fs::write(&path, contents).unwrap();
        let full = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
        let mut input = ReadInput::new("notes");
        input.limit = std::num::NonZeroUsize::new(1);
        let page = read.read(input, &cancel).await.unwrap();
        assert_eq!(page.data.revision, full.data.revision);
        assert_ne!(page.data.revision, first.data.revision);
        assert_eq!(page.text, "1: abc\n");
        let mut edit = EditInput::new("notes", "second", "updated");
        edit.expected_revision = Some(page.data.revision);
        let edited = EditTool::new(root.path())
            .await
            .unwrap()
            .edit(edit, &cancel)
            .await
            .unwrap();
        let reread = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
        assert_eq!(edited.data.revision, reread.data.revision);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            contents.replace("second", "updated")
        );
    }
}

#[tokio::test]
async fn edits_refuse_stale_revisions_even_when_the_target_text_still_matches() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes");
    std::fs::write(&path, "count=2\nowner=Alex\n").unwrap();
    let read = ReadTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let original = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, "count=2\nowner=Drew\n").unwrap();
    // Timestamp-only checks miss this external, same-size content change.
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(mtime))
        .unwrap();
    let tool = EditTool::new(root.path()).await.unwrap();
    let mut input = EditInput::new("notes", "count=2", "count=3");
    input.expected_revision = Some(original.data.revision);
    assert!(matches!(
        tool.edit(input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "count=2\nowner=Drew\n"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    let current = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
    input.expected_revision = Some(current.data.revision);
    let updated = tool.edit(input, &cancel).await.unwrap();
    let current = read.read(ReadInput::new("notes"), &cancel).await.unwrap();
    assert_eq!(updated.data.revision, current.data.revision);
    assert_eq!(current.text, "1: count=3\n2: owner=Drew\n");
}

#[tokio::test]
async fn guarded_writes_require_both_matching_revision_and_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let tool = WriteTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let created = tool
        .write(WriteInput::new("notes", "original"), &cancel)
        .await
        .unwrap();
    let mut input = WriteInput::new("notes", "changed");
    input.expected_revision = Some(created.data.revision.clone());
    assert!(matches!(
        tool.write(input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    input.overwrite = true;
    let changed = tool.write(input.clone(), &cancel).await.unwrap();
    assert_ne!(changed.data.revision, created.data.revision);
    assert!(matches!(
        tool.write(input.clone(), &cancel).await,
        Err(Error::Refused(_))
    ));
    input.expected_revision = Some(changed.data.revision.clone());
    let same = tool.write(input.clone(), &cancel).await.unwrap();
    assert!(!same.data.changed);
    assert_eq!(same.data.revision, changed.data.revision);
    std::fs::remove_file(root.path().join("notes")).unwrap();
    assert!(matches!(
        tool.write(input, &cancel).await,
        Err(Error::Refused(_))
    ));
    assert!(!root.path().join("notes").exists());
}

#[tokio::test]
async fn malformed_revisions_are_input_errors_and_never_mutate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes");
    std::fs::write(&path, "original").unwrap();
    let edit = EditTool::new(root.path()).await.unwrap();
    let write = WriteTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    for revision in [
        "",
        "sha256:abc",
        "mtime:123",
        &format!("sha256:{}", "g".repeat(64)),
    ] {
        let mut input = EditInput::new("notes", "original", "changed");
        input.expected_revision = Some(revision.to_owned());
        assert!(matches!(
            edit.edit(input, &cancel).await,
            Err(Error::InvalidInput(_))
        ));
        let mut input = WriteInput::new("notes", "changed");
        input.overwrite = true;
        input.expected_revision = Some(revision.to_owned());
        assert!(matches!(
            write.write(input, &cancel).await,
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }
}

#[tokio::test]
async fn a_later_independent_writer_refuses_a_revision_invalidated_by_an_edit() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes"), "one").unwrap();
    let cancel = CancellationToken::new();
    let revision = ReadTool::new(root.path())
        .await
        .unwrap()
        .read(ReadInput::new("notes"), &cancel)
        .await
        .unwrap()
        .data
        .revision;
    let first = EditTool::new(root.path()).await.unwrap();
    let second = WriteTool::new(root.path()).await.unwrap();
    let mut edit = EditInput::new("notes", "one", "two");
    edit.expected_revision = Some(revision.clone());
    first.edit(edit, &cancel).await.unwrap();
    let mut overwrite = WriteInput::new("notes", "three");
    overwrite.overwrite = true;
    overwrite.expected_revision = Some(revision);
    assert!(matches!(
        second.write(overwrite, &cancel).await,
        Err(Error::Refused(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("notes")).unwrap(),
        "two"
    );
}

#[tokio::test]
async fn shared_mutation_lock_covers_the_revision_check() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes"), "count=1\nowner=Alex\n").unwrap();
    let cancel = CancellationToken::new();
    let revision = ReadTool::new(root.path())
        .await
        .unwrap()
        .read(ReadInput::new("notes"), &cancel)
        .await
        .unwrap()
        .data
        .revision;
    let workspace = MutationWorkspace::new(root.path()).await.unwrap();
    let first = EditTool::from_workspace(workspace.clone());
    let second = EditTool::from_workspace(workspace);
    let mut a = EditInput::new("notes", "count=1", "count=2");
    let mut b = EditInput::new("notes", "owner=Alex", "owner=Drew");
    a.expected_revision = Some(revision.clone());
    b.expected_revision = Some(revision);
    let (a, b) = tokio::join!(first.edit(a, &cancel), second.edit(b, &cancel));
    assert!(matches!(
        (a, b),
        (Ok(_), Err(Error::Refused(_))) | (Err(Error::Refused(_)), Ok(_))
    ));
    let contents = std::fs::read_to_string(root.path().join("notes")).unwrap();
    assert!(matches!(
        contents.as_str(),
        "count=2\nowner=Alex\n" | "count=1\nowner=Drew\n"
    ));
}
