//! Read and verify a page of source text without RoomKit, Python or a model.
//! Run with `cargo run --example read_file` for the checked fixture, or
//! `cargo run --example read_file -- README.md` for a file under the current directory.

#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "the example shows the model-facing output"
)]

use std::error::Error;
use std::num::NonZeroUsize;
use std::process::ExitCode;

use roomkit_tools_fs::{ReadInput, ReadTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let mut paths = std::env::args().skip(1);
    if let Some(path) = paths.next() {
        if paths.next().is_some() {
            return Err("usage: read_file [path]".into());
        }
        let tool = ReadTool::new(std::env::current_dir()?).await?;
        let result = tool
            .read(ReadInput::new(path), &CancellationToken::new())
            .await?;
        print!("{}", result.text);
        println!("[Revision: {}]", result.data.revision);
        if let Some(offset) = result.data.next_offset {
            println!("[More lines remain; next offset: {offset}]");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join("hello.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )?;

    let tool = ReadTool::new(directory.path()).await?;
    let mut input = ReadInput::new("hello.rs");
    input.limit = Some(NonZeroUsize::MIN);
    let result = tool.read(input, &CancellationToken::new()).await?;
    print!("{}", result.text);

    Ok(
        if result.text.starts_with("1: fn main() {\n[next_offset=2,")
            && result.data.next_offset == Some(2)
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}
