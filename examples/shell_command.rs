//! Run a real foreground shell without an agent or model.
#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "an executable example displays its result"
)]

use roomkit_tools_process::{ShellInput, ShellTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("hours.txt"), "Saturday: 08:30-16:00\n")?;
    std::fs::write(
        root.path().join("orders.txt"),
        "43 | Taylor | 3 notebooks\n",
    )?;
    let result = ShellTool::new(root.path())
        .await?
        .shell(
            ShellInput::new("cat hours.txt orders.txt; printf 'two files read' >&2"),
            &CancellationToken::new(),
        )
        .await?;
    if result.data.exit_code != Some(0)
        || result.data.timed_out
        || !result.data.output_complete
        || result.data.stdout.text != "Saturday: 08:30-16:00\n43 | Taylor | 3 notebooks\n"
        || result.data.stderr.text != "two files read"
    {
        return Err("unexpected shell result".into());
    }
    println!("{}", result.text);
    Ok(())
}
