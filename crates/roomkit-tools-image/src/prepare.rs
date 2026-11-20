use std::borrow::Cow;
use std::io::{self, Cursor, Write};
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::{
    jpeg::JpegDecoder,
    png::{PngDecoder, PngEncoder},
    webp::WebPDecoder,
};
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageDecoder, ImageFormat, Limits};
use roomkit_tools_core::{Output, ToolError};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{ImageMime, ImagePayload, ViewImageLimits, ViewImageResult};

pub(crate) struct Control {
    pub token: CancellationToken,
    pub deadline: Instant,
}

impl Control {
    pub(crate) fn check(&self) -> Result<(), ToolError> {
        if self.token.is_cancelled() {
            Err(ToolError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(ToolError::LimitExceeded(
                "view_image deadline exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err owns decoder errors; diagnostic details are deliberately discarded"
)]
pub(crate) fn image_error(error: image::ImageError) -> ToolError {
    if matches!(error, image::ImageError::Limits(_)) {
        ToolError::LimitExceeded("image decoder resource limit exceeded".into())
    } else {
        // Decoder details may include arbitrary file metadata.
        ToolError::UnsupportedResource("invalid or unsupported image data".into())
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only three explicit formats are accepted, including after upstream adds formats"
)]
pub(crate) fn decoder<'a>(
    bytes: &'a [u8],
    bounds: &ViewImageLimits,
) -> Result<(ImageMime, Box<dyn ImageDecoder + 'a>), ToolError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(
        u32::try_from(bounds.max_dimension)
            .map_err(|_| ToolError::InvalidInput("max_dimension is too large".into()))?,
    );
    limits.max_image_height = limits.max_image_width;
    limits.max_alloc = Some(bounds.max_decode_bytes as u64);
    let cursor = Cursor::new(bytes);
    let (mime, mut decoder): (_, Box<dyn ImageDecoder>) =
        match image::guess_format(bytes).map_err(image_error)? {
            ImageFormat::Png => {
                let png = PngDecoder::with_limits(cursor, limits.clone()).map_err(image_error)?;
                if png.is_apng().map_err(image_error)? {
                    return Err(ToolError::UnsupportedResource(
                        "animated PNG is not supported; select a static frame first".into(),
                    ));
                }
                (ImageMime::Png, Box::new(png))
            }
            ImageFormat::Jpeg => (
                ImageMime::Jpeg,
                Box::new(JpegDecoder::new(cursor).map_err(image_error)?),
            ),
            ImageFormat::WebP => {
                let webp = WebPDecoder::new(cursor).map_err(image_error)?;
                if webp.has_animation() {
                    return Err(ToolError::UnsupportedResource(
                        "animated WebP is not supported; select a static frame first".into(),
                    ));
                }
                (ImageMime::Webp, Box::new(webp))
            }
            _ => {
                return Err(ToolError::UnsupportedResource(
                    "image input must be a static PNG, JPEG or WebP".into(),
                ));
            }
        };
    decoder.set_limits(limits).map_err(image_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(ToolError::UnsupportedResource(
            "image dimensions must be positive".into(),
        ));
    }
    if u64::from(width) * u64::from(height) > bounds.max_pixels as u64 {
        return Err(ToolError::LimitExceeded("image exceeds max_pixels".into()));
    }
    if decoder.total_bytes() > bounds.max_decode_bytes as u64 {
        return Err(ToolError::LimitExceeded(
            "decoded pixels exceed max_decode_bytes".into(),
        ));
    }
    Ok((mime, decoder))
}

struct BoundedWriter<'a> {
    bytes: Vec<u8>,
    maximum: usize,
    exceeded: bool,
    control: &'a Control,
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.control.check().map_err(io::Error::other)?;
        if buffer.len() > self.maximum.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("encoded image exceeds max_image_bytes"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// An encoded preview and its dimensions.
struct Preview {
    png: Vec<u8>,
    width: u32,
    height: u32,
    /// Smaller than the oriented source.
    resized: bool,
}

/// The largest preview of `image`, at most `edge` pixels on its longest side,
/// whose PNG fits `max_bytes`. Photographs compress poorly, so a preview over
/// the budget is made again with an edge three quarters as long, from the
/// source each time, down to one pixel.
fn fitting_preview(
    image: &DynamicImage,
    edge: u32,
    max_bytes: usize,
    control: &Control,
) -> Result<Preview, ToolError> {
    let longest = image.width().max(image.height());
    let mut target = edge.min(longest);
    loop {
        control.check()?;
        let resized = target < longest;
        let scaled =
            resized.then(|| image.resize(target, target, image::imageops::FilterType::Triangle));
        let preview = scaled.as_ref().unwrap_or(image);
        if let Some(png) = encode_png(preview, max_bytes, control)? {
            return Ok(Preview {
                png,
                width: preview.width(),
                height: preview.height(),
                resized,
            });
        }
        if target <= 1 {
            return Err(ToolError::LimitExceeded(format!(
                "the image's PNG preview exceeds the host's {max_bytes}-byte limit at every size"
            )));
        }
        target = (target - target / 4).min(target - 1);
    }
}

/// `image` as an interoperable 8-bit PNG, alpha preserved and metadata
/// stripped, or `None` once it passes `max_bytes`.
fn encode_png(
    image: &DynamicImage,
    max_bytes: usize,
    control: &Control,
) -> Result<Option<Vec<u8>>, ToolError> {
    control.check()?;
    let color = image.color();
    let normalized = if color == ColorType::Rgb8 || color == ColorType::Rgba8 {
        Cow::Borrowed(image)
    } else if color.has_alpha() {
        Cow::Owned(DynamicImage::ImageRgba8(image.to_rgba8()))
    } else {
        Cow::Owned(DynamicImage::ImageRgb8(image.to_rgb8()))
    };
    let mut output = BoundedWriter {
        bytes: Vec::new(),
        maximum: max_bytes,
        exceeded: false,
        control,
    };
    let encoded = normalized.write_with_encoder(PngEncoder::new(&mut output));
    control.check()?;
    if output.exceeded {
        return Ok(None);
    }
    encoded.map_err(image_error)?;
    Ok(Some(output.bytes))
}

pub(crate) fn prepare(
    path: String,
    bytes: &[u8],
    limits: ViewImageLimits,
    control: &Control,
) -> Result<Output<ViewImageResult>, ToolError> {
    control.check()?;
    let (mime, mut reader) = decoder(bytes, &limits)?;
    let (original_width, original_height) = reader.dimensions();
    let orientation = reader.orientation().map_err(image_error)?;
    control.check()?;
    let mut decoded = DynamicImage::from_decoder(reader).map_err(image_error)?;
    control.check()?;
    decoded.apply_orientation(orientation);
    let edge = u32::try_from(limits.max_edge)
        .map_err(|_| ToolError::InvalidInput("max_edge is too large".into()))?;
    let preview = fitting_preview(&decoded, edge, limits.max_image_bytes, control)?;
    drop(decoded);
    let resized = preview.resized;
    let result = ViewImageResult {
        path,
        revision: roomkit_tools_core::revision::sha256(&Sha256::digest(bytes)),
        source_mime_type: mime,
        input_bytes: bytes.len(),
        original_width,
        original_height,
        width: preview.width,
        height: preview.height,
        orientation_applied: orientation != Orientation::NoTransforms,
        resized,
        output_bytes: preview.png.len(),
        image: ImagePayload {
            mime_type: ImageMime::Png,
            base64: STANDARD.encode(&preview.png),
        },
    };
    control.check()?;
    let text = format!(
        "Image {:?}: {}x{} {} -> {}x{} PNG preview; orientation_applied={}, resized={}, output_bytes={}. Pixels are attached separately.\n",
        result.path,
        original_width,
        original_height,
        mime.as_str(),
        result.width,
        result.height,
        result.orientation_applied,
        resized,
        result.output_bytes
    );
    Ok(Output::new(result, text))
}
