//! Compare, preview and apply opening hours without RoomKit or an external model.
#![expect(
    clippy::print_stdout,
    reason = "the executable example shows its checked results"
)]

use roomkit_tools_fs::{
    DiffInput, DiffTool, MutationWorkspace, PatchChange, PatchInput, PatchTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let old = "Saturday: 08:30-16:00\n";
    let new = "Saturday: 08:30-17:00\n";
    std::fs::write(root.path().join("hours.txt"), old)?;
    std::fs::write(root.path().join("proposed.txt"), new)?;
    let token = CancellationToken::new();
    let diff = DiffTool::new(root.path()).await?;
    let patch = PatchTool::from_workspace(MutationWorkspace::new(root.path()).await?);
    let before = diff
        .diff(DiffInput::new("hours.txt", "proposed.txt"), &token)
        .await?;
    if before.data.additions != 1 || before.data.deletions != 1 {
        return Err("unexpected diff".into());
    }
    println!("{}", before.text);
    let mut input = PatchInput::new(vec![
        PatchChange::Update {
            path: "hours.txt".into(),
            old_content: old.into(),
            new_content: new.into(),
        },
        PatchChange::Add {
            path: "notice.txt".into(),
            content: "Saturday closing is now 17:00.\n".into(),
        },
    ]);
    let preview = patch.patch(input.clone(), &token).await?;
    if !preview.data.dry_run
        || !preview.data.applied.is_empty()
        || std::fs::read_to_string(root.path().join("hours.txt"))? != old
    {
        return Err("preview wrote files".into());
    }
    input.dry_run = false;
    let applied = patch.patch(input, &token).await?;
    if !applied.data.completed || applied.data.applied.len() != 2 {
        return Err("patch was not completed".into());
    }
    let after = diff
        .diff(DiffInput::new("hours.txt", "proposed.txt"), &token)
        .await?;
    if after.data.changed {
        return Err("files still differ".into());
    }
    println!("Two changes applied; hours.txt matches proposed.txt.");
    Ok(())
}
