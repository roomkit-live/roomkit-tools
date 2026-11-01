# Local image viewing

`view_image` is independent of `read`, which keeps its text-only contract,
and does not require Python imaging
libraries or a model provider for standalone preparation.

Select it directly in a RoomKit Python agent with a vision-capable provider:

```python
from roomkit import AIChannel
from roomkit_tools import ViewImage

agent = AIChannel(
    "assistant",
    provider=vision_provider,  # A configured RoomKit provider/model accepting images.
    tools=[ViewImage(root="./workspace", max_edge=2048)],
)
```

The model calls `view_image` with `{"path": "screenshot.png"}` and an optional
`timeout_ms`. The adapter returns `AITextPart` metadata and an `AIImagePart`
containing actual pixels. RoomKit performs provider-specific multimodal encoding;
base64 is not included in ordinary tool text. This tool supplies pixels, not OCR
or a generated caption. The host must select a model that accepts images.

Without RoomKit, inside an async function:

```python
from roomkit_tools import ViewImageTool

tool = await ViewImageTool.create("./workspace", max_edge=2048)
result = await tool.view("screenshot.png")
metadata = result["data"]
pixels = metadata["image"]  # mime_type and complete base64; deliver as image content.
```

Rust hosts use `ViewImageTool::new(root).await?`, optionally `with_limits(...)`,
then `view(ViewImageInput { path, ..Default::default() }, &cancel).await?`.
`result.data.image.data_url()` provides a data URL for multimodal transport.
The pinned optional RoomKit **Rust** adapter currently has only text/JSON tool
results, so it does not yet expose an image toolset.

Static PNG, JPEG and WebP are detected from bytes, independently of extensions.
The tool applies EXIF orientation, reduces oversized images while preserving
aspect ratio, and re-encodes to 8-bit PNG, preserving alpha and stripping source
metadata. It does not upscale or perform color-profile conversion. Animated
PNG/WebP, GIF, SVG, PDF and remote URLs are unsupported. The source is unchanged.
Results include its workspace-relative `path` (the host's absolute path is not
returned), its SHA-256 `revision`, original and preview dimensions,
`orientation_applied`, `resized`, and byte counts. A preview that cannot fit the
output budget fails explicitly; it is never returned as a partial image.

Host defaults are 10 MiB input, 16,384 pixels per source edge, 16,777,216 source
pixels, 128 MiB of best-effort decoder allocations, a 2,048-pixel preview edge,
and 4 MiB of encoded PNG output (before base64 expansion). The default deadline
is 10 seconds, including queueing and I/O, with a 120-second per-call maximum.
These are configurable host limits. The decoder allocation setting is not a
total process memory quota. Two worker slots are shared by clones of each tool;
cancellation/deadlines return promptly, but decode/resize cannot be interrupted
mid-stage and retain their slot until they stop.

Peak memory per slot exceeds `max_decode_bytes`: the decoded image, an
orientation copy and the resize filter's 32-bit float intermediate can be alive
together. Measured peak growth for 4096×4096 sources at the defaults (VmHWM,
Linux x86-64 release build):

| Source | Decoded | Peak |
| --- | --- | --- |
| 8-bit RGB PNG, or JPEG with EXIF rotation | 48 MiB | 189 MiB |
| 8-bit RGBA PNG | 64 MiB | 208 MiB |
| 16-bit RGBA PNG (the `max_decode_bytes` limit) | 128 MiB | 289 MiB |

Budget about 2.25 × `max_decode_bytes` per slot, so roughly 580 MiB with both
slots busy at the defaults. A path that lexically leaves
the root is refused before any filesystem lookup; symlinks are checked after
canonicalization, and a path that a symlink takes outside the root is refused
whether or not it exists there. Root containment assumes a host-controlled
workspace, not hostile concurrent path replacement or an OS sandbox.

`cargo run --example view_image` checks native image preparation.
`python examples/roomkit_image.py` checks actual multimodal delivery with a
scripted provider; it makes no visual-recognition claim. With the vLLM extra
installed, `OPENAI_BASE_URL`, `OPENAI_MODEL` and optionally `OPENAI_API_KEY` set,
add `--live` to ask a vision model to describe a synthetic image. Its contents
are not given in the prompt; the host prints a reference afterward for comparison.

[Documentation index](README.md) · [Project overview](../README.md)
