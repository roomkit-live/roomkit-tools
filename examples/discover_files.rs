//! Discover and search text files using independent tools, without a model.

#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "the example displays the tool results")]

use std::error::Error;
use std::process::ExitCode;

use roomkit_tools_fs::{
    GlobInput, GlobTool, GrepInput, GrepTool, ListInput, ListTool, ReadInput, ReadTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join("info"))?;
    std::fs::write(
        directory.path().join("info/hours.txt"),
        "Opening hours\nSaturday: 08:30-16:00\n",
    )?;
    let cancel = CancellationToken::new();
    let listing = ListTool::new(directory.path())
        .await?
        .list(ListInput::default(), &cancel)
        .await?;
    print!("list:\n{}", listing.text);
    let paths = GlobTool::new(directory.path())
        .await?
        .glob(GlobInput::new("**/*.txt"), &cancel)
        .await?;
    print!("glob:\n{}", paths.text);
    let mut search = GrepInput::new("Saturday");
    search.context_before = 1;
    let matches = GrepTool::new(directory.path())
        .await?
        .grep(search, &cancel)
        .await?;
    print!("grep:\n{}", matches.text);
    let Some(found) = paths.data.entries.first() else {
        return Ok(ExitCode::FAILURE);
    };
    let read = ReadTool::new(directory.path())
        .await?
        .read(ReadInput::new(&found.path), &cancel)
        .await?;
    print!("read:\n{}", read.text);
    Ok(
        if listing.data.entries.len() == 1
            && matches.data.matches.len() == 1
            && matches.data.matches.first().is_some_and(|found| {
                found
                    .context_before
                    .first()
                    .is_some_and(|line| line.text == "Opening hours")
            })
            && read.text == "1: Opening hours\n2: Saturday: 08:30-16:00\n"
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}
