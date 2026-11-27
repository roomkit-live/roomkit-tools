#![forbid(unsafe_code)]
//! The same host-visible calls against independent handles to one directory.
use roomkit_tools_memory::{FileMemory, MemoryInput, MemoryTool};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
fn save(id: &str, text: &str) -> MemoryInput {
    MemoryInput {
        action: "save".into(),
        id: Some(id.into()),
        title: Some("Windows runner".into()),
        text: Some(text.into()),
        ..Default::default()
    }
}
#[tokio::test]
async fn notes_are_searchable_conditional_and_quota_checked_across_handles() -> Result {
    let dir = tempfile::tempdir()?;
    let store = || MemoryTool::new(Arc::new(FileMemory::new(dir.path().to_owned(), 256)));
    let (left, right) = (store(), store());
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(
        left.call(save("load", "Use the blue runner"), &cancel),
        right.call(save("load", "Use the green runner"), &cancel)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let row = a.or(b)?.data.notes.remove(0);
    let search = MemoryInput {
        action: "search".into(),
        query: Some("runner load".into()),
        ..Default::default()
    };
    let result = right.call(search, &cancel).await?;
    assert_eq!(result.data.notes.len(), 1);
    assert!(result.data.notes[0].text.is_none());
    assert!(right.call(save("../escape", "bad"), &cancel).await.is_err());
    assert!(
        right
            .call(save("memory", "reserved index"), &cancel)
            .await
            .is_err()
    );
    assert!(
        right
            .call(save("oversize", &"x".repeat(240)), &cancel)
            .await
            .is_err()
    );
    let mut update = save("load", "Run on the yellow host");
    update.revision = Some(row.revision.clone());
    let changed = right.call(update, &cancel).await?.data.notes.remove(0);
    let mut forget = MemoryInput {
        action: "forget".into(),
        id: Some("load".into()),
        revision: Some(row.revision),
        ..Default::default()
    };
    assert!(left.call(forget.clone(), &cancel).await.is_err());
    forget.revision = Some(changed.revision);
    left.call(forget, &cancel).await?;
    assert!(!dir.path().join("load.md").exists());
    assert!(!std::fs::read_to_string(dir.path().join("MEMORY.md"))?.contains("load.md"));
    Ok(())
}

#[tokio::test]
async fn index_repairs_on_reopen_and_smaller_quotas_allow_cleanup() -> Result {
    let root = tempfile::tempdir()?;
    let cancel = CancellationToken::new();
    let store = |quota| MemoryTool::new(Arc::new(FileMemory::new(root.path().into(), quota)));
    let wide = store(8192);
    let entry = wide
        .call(save("first", &"a".repeat(200)), &cancel)
        .await?
        .data
        .notes
        .remove(0);
    wide.call(save("second", &"b".repeat(200)), &cancel).await?;
    std::fs::write(root.path().join("MEMORY.md"), "old index")?;
    let narrow = store(64);
    let listed = narrow
        .call(
            MemoryInput {
                action: "search".into(),
                ..Default::default()
            },
            &cancel,
        )
        .await?;
    assert_eq!(listed.data.notes.len(), 2);
    assert!(std::fs::read_to_string(root.path().join("MEMORY.md"))?.contains("second.md"));
    narrow
        .call(
            MemoryInput {
                action: "forget".into(),
                id: Some(entry.id),
                revision: Some(entry.revision),
                ..Default::default()
            },
            &cancel,
        )
        .await?;
    assert!(!root.path().join("first.md").exists());
    Ok(())
}
