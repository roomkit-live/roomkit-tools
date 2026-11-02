# roomkit-tools-ocr-rusto

A local OCR engine for `roomkit-tools-ocr`, built on
[RustO](https://crates.io/crates/rusto-rs) and RTen: PP-OCRv6 detection and
recognition in pure Rust. Models are trusted host files; nothing is
downloaded. Supply a directory holding `det.onnx`, `rec.onnx` and `dict.txt`
(see the [OCR guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/ocr.md)). Inference runs on thread pools private
to the engine.

## Features

None.

## Example

```rust,no_run
use roomkit_tools_ocr::{OcrInput, OcrTool};
use roomkit_tools_ocr_rusto::{RustoConfig, RustoOcr};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Loaded on the first call, on the engine's own worker.
    let engine = RustoOcr::lazy(RustoConfig::new("models"))?;
    let ocr = OcrTool::new("documents", engine).await?;
    let result = ocr
        .ocr(OcrInput::new("scan.png"), &CancellationToken::new())
        .await?;
    print!("{}", result.text);
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
