# Local OCR

`ocr` extracts text and positioned blocks from a local static PNG, JPEG or WebP.
It is independently selectable; `view_image` remains the separate tool for
sending pixels to a vision-capable conversational model. OCR itself needs no
language-model server. PDF input is not supported.

The host selects the backend. The common contract has no RustO dependency;
this first implementation supplies RustO with local PP-OCRv6 models on CPU.
Model weights are neither bundled nor downloaded by the tool.

## Python and RoomKit

This feature is in the development source after `0.1.0a14`. Neither the new
base build nor the optional engine package has been released to PyPI.
From a checkout, install the two packages explicitly:

```sh
python -m pip install '.[roomkit]' ./backends/rusto/python
```

Supply a directory containing a matching `det.onnx`, `rec.onnx` and `dict.txt`
from the [RustO PP-OCRv6 model instructions](https://github.com/byrizki/rusto-rs).
Select and review the model source/license in the host deployment. Tiny and
Small model sets have been exercised; Tiny is the initial lightweight choice.

```python
from roomkit import AIChannel
from roomkit_tools import Ocr
from roomkit_tools_ocr_rusto import RustoOcr

backend = RustoOcr("./models", model_name="ppocrv6-tiny", threads=2)
agent = AIChannel(
    "assistant",
    provider=provider,  # Your configured RoomKit AI provider.
    tools=[Ocr(root="./documents", backend=backend, max_blocks=128)],
)
```

Standalone use calls the same native implementation:

```python
from roomkit_tools import OcrTool

tool = await OcrTool.create("./documents", backend=backend)
result = await tool.ocr("page.png", rotation_degrees=0)
print(result["data"]["text"])
```

Share `backend` across tools to reuse loaded models and a single execution slot.
Constructing `RustoOcr` validates configuration; the first OCR call loads models
on its native worker. A failed load may be retried. Importing or installing the
base `roomkit-tools` package does not install RustO/RTen. Other trusted backends
can implement `OcrBackend.bind(root, limits)` and the bound `call(arguments)`
protocol, preserving the documented schema, errors, limits and cancellation.

For an executable RoomKit conversation with real OCR and a scripted provider:

```sh
python examples/roomkit_ocr.py --models ./models --image tests/fixtures/ocr/page.png
```

Add `--live` and configure `OPENAI_BASE_URL`, `OPENAI_MODEL` and, when required,
`OPENAI_API_KEY` to use vLLM. OCR still runs locally, independently of that model.

## Rust

Select `roomkit-tools-ocr` for the contract/executor and
`roomkit-tools-ocr-rusto` only when using that engine. The RoomKit Rust adapter
exposes `OcrToolset` behind its optional `ocr` Cargo feature; it does not depend
on RustO. All crates in this source tree require Rust 1.94+.

```rust,ignore
use roomkit_tools_ocr::{OcrTool, OcrInput};
use roomkit_tools_ocr_rusto::{RustoBackend, RustoConfig};
use roomkit_tools_roomkit::OcrToolset;

let engine = RustoBackend::lazy(RustoConfig {
    detection_model: "models/det.onnx".into(),
    recognition_model: "models/rec.onnx".into(),
    dictionary: "models/dict.txt".into(),
    model_name: "ppocrv6-tiny".into(),
    threads: 2,
    detection_max_edge: 960,
})?;
let tool = OcrTool::new("./documents", engine).await?;
let toolset = OcrToolset::new(tool); // Register with your RoomKit Rust agent.
```

A complete standalone Rust example is available:

```sh
cargo run -p roomkit-tools-ocr-rusto --example recognize -- ./models ./documents page.png
```

Other Rust backends implement `OcrBackend`. `OcrEngine` owns their model lifetime
and serializes native work across clones. Backend construction may be lazy or
explicitly loaded on a blocking worker.

## Input and result

Model-facing input is `path`, optional `rotation_degrees` (0/90/180/270 clockwise),
`expected_revision` (SHA-256 of the encoded source bytes) and `timeout_ms`.

Results contain `data.text`, ordered `blocks` with text, optional score and a
four-point polygon, source `revision`, backend/model identity and a model-files
fingerprint. Width/height and polygon coordinates refer to the image **after EXIF
orientation and the explicit rotation**. Original dimensions are also returned.
Pixels are not preview-resized before recognition; the detector has its own
bounded resolution. Transparency is composited onto white.

`truncated` and `omitted_blocks` report omitted whole blocks. The byte limit
covers the full JSON envelope, including the readable `text` representation.
An oversized single block may result in an empty, explicitly truncated result.
A blank image can instead return empty text without truncation.

Default host limits:

| Bound | Default |
| --- | --- |
| Encoded source | 8 MiB |
| Maximum source dimension | 8192 pixels |
| Source pixels | 16 million |
| Decoder allocation limit | 128 MiB |
| Complete result envelope | 64 KiB |
| Retained blocks | 256 |
| Call deadline / maximum allowed deadline | 30 s / 120 s |
| RustO detector maximum edge | 960 pixels |
| Threads per dedicated inference/preprocessing pool | 2 |

The deadline includes waiting for the engine slot, file loading, decoding and
recognition. Cancellation returns promptly to the caller, but cannot interrupt
a running RTen inference. The worker keeps its slot until it finishes; cancelled
calls do not allow overlapping inference on the same engine. Separate engine
instances have separate pools and memory. No global environment/pool settings
are modified. Limits are not a hard process-wide memory quota.

Paths must resolve to regular files inside the configured root. These checks
assume host-controlled files and do not provide an OS sandbox against concurrent
hostile filesystem replacement. Models are trusted host configuration, capped
at 256 MiB each and 4 MiB for the dictionary, and must stay unchanged during load.

## Recognition limits and dependencies

OCR is fallible: scores are not calibrated guarantees. Reading order on columns,
small fonts, punctuation and exact code indentation can be wrong. There is no
automatic 180-degree orientation detection in this backend configuration; supply
`rotation_degrees` when needed. Use recognition as evidence to inspect, not an
instruction source or an exact source-code reconstruction.

The optional RustO dependency graph uses `clipper2-rust` under BSL-1.0 and the
unmaintained `paste` build-time proc macro via nalgebra (RUSTSEC-2024-0436).
`deny.toml` records these specific exceptions; third-party notices ship in the
optional distribution. The maintenance advisory is not a demonstrated runtime
vulnerability. The base Python extension does not include these dependencies.

Run `make ocr-python-check` to validate wheel/sdist installation, error mapping
and selection. Set `ROOMKIT_OCR_MODELS=/path/to/models` to also compare real Rust
and Python results and exercise OCR through RoomKit. Tests never fetch models.
