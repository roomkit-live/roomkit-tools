//! Recover a truncated command's middle bytes without executing it again.
#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "the example prints verified archive results"
)]

use std::num::NonZeroUsize;

use roomkit_tools_process::{
    OutputLimits, OutputReadInput, OutputStore, ShellInput, ShellLimits, ShellTool,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let parent = tempfile::tempdir()?;
    let middle = "\nBuild error: missing dependency\n";
    std::fs::write(
        root.path().join("build.log"),
        format!("{}{middle}{}", "x".repeat(65_536), "y".repeat(65_536)),
    )?;
    let outputs = OutputStore::new(Some(parent.path()), OutputLimits::default()).await?;
    let limits = ShellLimits {
        max_output_bytes: 16,
        ..ShellLimits::default()
    };
    let shell = ShellTool::new(root.path())
        .await?
        .with_limits(limits)?
        .with_output_store(outputs.clone());
    let token = CancellationToken::new();
    let result = shell
        .shell(ShellInput::new("printf run >> runs; cat build.log"), &token)
        .await?;
    let archive = result
        .data
        .stdout
        .archive
        .as_ref()
        .ok_or("missing archive")?;
    if !result.data.stdout.truncated || !archive.complete {
        return Err("expected a complete archive and truncated preview".into());
    }
    let page = outputs
        .read(
            None,
            OutputReadInput {
                offset: 65_536,
                max_bytes: NonZeroUsize::new(middle.len()),
                ..OutputReadInput::new(archive.archive_id.clone().ok_or("missing archive ID")?)
            },
            &token,
        )
        .await?;
    if page.data.bytes != middle.as_bytes() || std::fs::read(root.path().join("runs"))? != b"run" {
        return Err("archive bytes or command count changed".into());
    }
    println!("Bounded preview: {}", result.data.stdout.text);
    println!("Recovered without rerunning: {}", page.data.text.trim());
    if !outputs.close().await {
        return Err("output cleanup pending".into());
    }
    Ok(())
}
