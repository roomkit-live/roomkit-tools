//! Create, add, update and remove customer orders with independent Rust tools.

#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "the example displays completed mutations"
)]

use std::error::Error;
use std::process::ExitCode;

use roomkit_tools_fs::{
    EditInput, EditTool, ReadInput, ReadTool, Workspace, WriteInput, WriteTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let workspace = Workspace::new(root.path()).await?;
    let write = WriteTool::from_workspace(workspace.clone());
    let edit = EditTool::from_workspace(workspace);
    let cancel = CancellationToken::new();
    let result = write
        .write(
            WriteInput::new("orders.txt", "Orders\n43 | Taylor | 2 notebooks\n"),
            &cancel,
        )
        .await?;
    print!("{}", result.text);
    let mut revision = result.data.revision;
    for (old, new) in [
        ("Orders\n", "Orders\n44 | Alice | 1 pen\n"),
        ("43 | Taylor | 2 notebooks", "43 | Taylor | 3 notebooks"),
        ("44 | Alice | 1 pen\n", ""),
    ] {
        let mut input = EditInput::new("orders.txt", old, new);
        input.expected_revision = Some(revision);
        let result = edit.edit(input, &cancel).await?;
        revision = result.data.revision;
        print!("{}", result.text);
    }
    let read = ReadTool::new(root.path())
        .await?
        .read(ReadInput::new("orders.txt"), &cancel)
        .await?;
    print!("{}", read.text);
    Ok(
        if read.text == "1: Orders\n2: 43 | Taylor | 3 notebooks\n"
            && read.data.revision == revision
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}
