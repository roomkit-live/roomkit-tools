//! Opt-in real-model smoke test. Models are explicit local host inputs.
#![allow(
    clippy::print_stdout,
    reason = "example emits the observed tool envelope"
)]
use roomkit_tools_ocr::{OcrInput, OcrTool};
use roomkit_tools_ocr_rusto::{RustoBackend, RustoConfig};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let models = std::path::PathBuf::from(args.next().ok_or("expected models directory")?);
    let root = args.next().ok_or("expected image workspace")?;
    let path = args.next().ok_or("expected image path")?;
    let rotation_degrees = args.next().map_or(Ok(0), |arg| arg.parse::<u16>())?;
    let engine = RustoBackend::lazy(RustoConfig {
        detection_model: models.join("det.onnx"),
        recognition_model: models.join("rec.onnx"),
        dictionary: models.join("dict.txt"),
        model_name: "ppocrv6".into(),
        threads: 2,
        detection_max_edge: 960,
    })?;
    let tool = OcrTool::new(root, engine).await?;
    let output = tool
        .ocr(
            OcrInput {
                path,
                rotation_degrees,
                expected_revision: None,
                timeout_ms: None,
            },
            &CancellationToken::new(),
        )
        .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
