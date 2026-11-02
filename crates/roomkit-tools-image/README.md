# roomkit-tools-image

The `view_image` tool: decodes a local PNG, JPEG or WebP beneath the
workspace root within host bounds, applies EXIF orientation and returns a
normalized PNG preview with its metadata. Deliver the preview to the model as
image content: serialized into text, it does not give the model vision.

## Features

None. PNG, JPEG and WebP decoding are always included.

## Example

```rust
use roomkit_tools_image::{ViewImageInput, ViewImageTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30]))
        .save(root.path().join("swatch.png"))?;

    let tool = ViewImageTool::new(root.path()).await?;
    let result = tool
        .view(ViewImageInput::new("swatch.png"), &CancellationToken::new())
        .await?;
    assert_eq!((result.data.width, result.data.height), (4, 3));
    // result.data.image holds the base64 PNG to send as image content.
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
