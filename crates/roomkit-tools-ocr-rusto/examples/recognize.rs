//! Opt-in real-model smoke test. Models are explicit local host inputs.
#![allow(
    clippy::print_stdout,
    reason = "example emits the observed tool envelope"
)]
use roomkit_tools_ocr::{OcrInput, OcrTool};
use roomkit_tools_ocr_rusto::{RustoConfig, RustoOcr};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let models = std::path::PathBuf::from(args.next().ok_or("expected models directory")?);
    let root = args.next().ok_or("expected image workspace")?;
    let path = args.next().ok_or("expected image path")?;
    let rotation_degrees = args.next().map_or(Ok(0), |arg| arg.parse::<u16>())?;
    let engine = RustoOcr::lazy(RustoConfig::new(models))?;
    let tool = OcrTool::new(root, engine).await?;
    let output = tool
        .ocr(
            OcrInput {
                rotation_degrees,
                ..OcrInput::new(path)
            },
            &CancellationToken::new(),
        )
        .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
