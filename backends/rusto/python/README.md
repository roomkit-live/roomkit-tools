# roomkit-tools-ocr-rusto

Optional CPU OCR backend for `roomkit-tools`, using RustO and RTen.
Install this package only when selecting OCR. It ships no model weights and
never downloads them automatically. The host supplies matching PP-OCRv6
`det.onnx`, `rec.onnx` and `dict.txt` files.

```python
from roomkit_tools import Ocr
from roomkit_tools_ocr_rusto import RustoOcr

tools = [Ocr(root="./documents", backend=RustoOcr("./models"))]
# Pass tools to RoomKit's AIChannel, or use OcrTool.create for standalone calls.
```

Prebuilt wheels and source archives are distributed through
[GitHub previews](https://github.com/roomkit-live/roomkit-tools/releases).
Install the matching base package alongside this optional backend. From a checkout:

```sh
uv pip install . ./backends/rusto/python
```

Source builds require Rust 1.94+. Recognition runs locally on CPU, with two
threads per dedicated inference/preprocessing pool by default. Share a
`RustoOcr` instance to reuse its models and one execution slot across tools.
Loading is deferred to the first call's native worker. Cancelling an awaiting
call does not interrupt a running native inference; its slot remains occupied
until the worker ends.

Supports static PNG/JPEG/WebP, EXIF orientation and explicit clockwise rotation.
No PDFs, automatic page orientation, guaranteed reading order or exact code
indentation. Scores do not prove accuracy. Model paths are trusted host inputs
and must stay unchanged during loading. Review model licensing separately.

The optional dependency graph includes `clipper2-rust` (BSL-1.0) and the
unmaintained `paste` build-time macro (RUSTSEC-2024-0436), with narrow documented
repository policy exceptions. Third-party notices are included in the distribution.

Maintainers: regenerate native notices after dependency changes with
`python backends/rusto/python/update_notices.py` from the repository root.
