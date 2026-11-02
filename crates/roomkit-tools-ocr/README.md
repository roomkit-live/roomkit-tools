# roomkit-tools-ocr

The `ocr` tool: transcribes a local PNG, JPEG or WebP beneath the workspace
root into text blocks with their positions, within host bounds. Recognition
runs in an `OcrEngine` the host loads once and shares between tools; the
engine wraps an `OcrBackend`, such as the one in `roomkit-tools-ocr-rusto`.
Cancellation releases the caller at once, while native work already started
finishes on its worker.

## Features

None.

## Example

```rust
use roomkit_tools_ocr::{OcrEngine, OcrInput, OcrTool};
use tokio_util::sync::CancellationToken;

/// Transcribe `scan.png` with an engine the host loaded once, for example
/// `roomkit_tools_ocr_rusto::RustoOcr::lazy`.
async fn transcribe(engine: OcrEngine) -> Result<String, Box<dyn std::error::Error>> {
    let ocr = OcrTool::new("documents", engine).await?;
    let result = ocr
        .ocr(OcrInput::new("scan.png"), &CancellationToken::new())
        .await?;
    Ok(result.text)
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
