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
use image::{DynamicImage, ImageDecoder, ImageFormat, Limits};
use roomkit_tools_core::{Error, Output};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{ImageMime, ImagePayload, ViewImageLimits, ViewImageResult};

pub(crate) struct Control {
    pub token: CancellationToken,
    pub deadline: Instant,
}

impl Control {
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.token.is_cancelled() {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::LimitExceeded("view_image deadline exceeded".into()))
        } else {
            Ok(())
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err owns decoder errors; diagnostic details are deliberately discarded"
)]
pub(crate) fn image_error(error: image::ImageError) -> Error {
    if matches!(error, image::ImageError::Limits(_)) {
        Error::LimitExceeded("image decoder resource limit exceeded".into())
    } else {
        // Decoder details may include arbitrary file metadata.
        Error::Unsupported("invalid or unsupported image data".into())
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only three explicit formats are accepted, including after upstream adds formats"
)]
pub(crate) fn decoder<'a>(
    bytes: &'a [u8],
    bounds: &ViewImageLimits,
) -> Result<(ImageMime, Box<dyn ImageDecoder + 'a>), Error> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(
        u32::try_from(bounds.max_dimension)
            .map_err(|_| Error::InvalidInput("max_dimension is too large".into()))?,
    );
    limits.max_image_height = limits.max_image_width;
    limits.max_alloc = Some(bounds.max_decode_bytes as u64);
    let cursor = Cursor::new(bytes);
    let (mime, mut decoder): (_, Box<dyn ImageDecoder>) =
        match image::guess_format(bytes).map_err(image_error)? {
            ImageFormat::Png => {
                let png = PngDecoder::with_limits(cursor, limits.clone()).map_err(image_error)?;
                if png.is_apng().map_err(image_error)? {
                    return Err(Error::Unsupported(
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
                    return Err(Error::Unsupported(
                        "animated WebP is not supported; select a static frame first".into(),
                    ));
                }
                (ImageMime::Webp, Box::new(webp))
            }
            _ => {
                return Err(Error::Unsupported(
                    "image input must be a static PNG, JPEG or WebP".into(),
                ));
            }
        };
    decoder.set_limits(limits).map_err(image_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Unsupported(
            "image dimensions must be positive".into(),
        ));
    }
    if u64::from(width) * u64::from(height) > bounds.max_pixels as u64 {
        return Err(Error::LimitExceeded("image exceeds max_pixels".into()));
    }
    if decoder.total_bytes() > bounds.max_decode_bytes as u64 {
        return Err(Error::LimitExceeded(
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

pub(crate) fn prepare(
    path: String,
    bytes: &[u8],
    limits: ViewImageLimits,
    control: &Control,
) -> Result<Output<ViewImageResult>, Error> {
    control.check()?;
    let (mime, mut reader) = decoder(bytes, &limits)?;
    let (original_width, original_height) = reader.dimensions();
    let orientation = reader.orientation().map_err(image_error)?;
    control.check()?;
    let mut decoded = DynamicImage::from_decoder(reader).map_err(image_error)?;
    control.check()?;
    decoded.apply_orientation(orientation);
    let edge = u32::try_from(limits.max_edge)
        .map_err(|_| Error::InvalidInput("max_edge is too large".into()))?;
    let resized = decoded.width().max(decoded.height()) > edge;
    if resized {
        decoded = decoded.resize(edge, edge, image::imageops::FilterType::Triangle);
    }
    control.check()?;
    // Normalize to an interoperable 8-bit PNG; preserve alpha, strip metadata.
    let normalized = if decoded.color().has_alpha() {
        DynamicImage::ImageRgba8(decoded.into_rgba8())
    } else {
        DynamicImage::ImageRgb8(decoded.into_rgb8())
    };
    let mut output = BoundedWriter {
        bytes: Vec::new(),
        maximum: limits.max_image_bytes,
        exceeded: false,
        control,
    };
    let encoded = normalized.write_with_encoder(PngEncoder::new(&mut output));
    control.check()?;
    if output.exceeded {
        return Err(Error::LimitExceeded(
            "PNG preview exceeds max_image_bytes; reduce max_edge or raise the host bound".into(),
        ));
    }
    encoded.map_err(image_error)?;
    let result = ViewImageResult {
        path,
        revision: format!("sha256:{:x}", Sha256::digest(bytes)),
        source_mime_type: mime,
        input_bytes: bytes.len(),
        original_width,
        original_height,
        width: normalized.width(),
        height: normalized.height(),
        orientation_applied: orientation != Orientation::NoTransforms,
        resized,
        output_bytes: output.bytes.len(),
        image: ImagePayload {
            mime_type: ImageMime::Png,
            base64: STANDARD.encode(&output.bytes),
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
