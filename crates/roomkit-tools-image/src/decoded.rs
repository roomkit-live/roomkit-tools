use std::path::{Path, PathBuf};

use image::{DynamicImage, metadata::Orientation};
use roomkit_tools_core::{ToolError, WorkspaceRoot};

use crate::{ImageMime, ViewImageLimits, prepare, source};

/// Canonical workspace for tools that consume local image bytes.
#[derive(Clone, Debug)]
pub struct ImageWorkspace {
    root: WorkspaceRoot,
}

impl ImageWorkspace {
    /// Resolve an existing host-controlled directory.
    ///
    /// # Errors
    /// Returns an error if the root cannot be resolved to a directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: source::root(root.as_ref()).await?,
        })
    }

    /// Load a regular image file with a byte cap and canonical containment check.
    /// No hostile concurrent filesystem replacement protection is promised.
    ///
    /// # Errors
    /// Rejects invalid paths, escapes, non-files, oversized inputs and I/O failures.
    pub async fn load(&self, path: &str, maximum: usize) -> Result<(PathBuf, Vec<u8>), ToolError> {
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || maximum == 0
            || maximum == usize::MAX
        {
            return Err(ToolError::InvalidInput(
                "invalid image path or byte limit".into(),
            ));
        }
        source::load(&self.root, path, maximum).await
    }

    /// Name a path returned by [`ImageWorkspace::load`] relative to the root,
    /// with `.` for the root itself, so results never expose host paths.
    ///
    /// # Errors
    /// Refuses a path outside the root and rejects a non-UTF-8 name.
    pub fn relative(&self, path: &Path) -> Result<String, ToolError> {
        self.root.relative(path)
    }
}

/// Full-resolution, oriented RGB pixels. Alpha is composited on white.
#[derive(Debug)]
pub struct DecodedImage {
    /// Original encoded format.
    pub mime: ImageMime,
    /// Width before orientation.
    pub original_width: u32,
    /// Height before orientation.
    pub original_height: u32,
    /// Whether EXIF orientation was applied.
    pub orientation_applied: bool,
    /// Width after EXIF and explicit clockwise rotation.
    pub width: u32,
    /// Height after EXIF and explicit clockwise rotation.
    pub height: u32,
    /// Packed RGB bytes, without row padding.
    pub pixels: Vec<u8>,
}

/// Decode a static PNG/JPEG/WebP without preview downscaling.
/// Only the dimension, pixel and decoded-byte bounds are used from `limits`.
/// Rotation must be 0, 90, 180 or 270 degrees clockwise, after EXIF orientation.
/// Decoder bounds do not constitute a hard process-wide memory quota.
///
/// # Errors
/// Rejects invalid rotations, unsupported/corrupt images and exceeded decode limits.
pub fn decode_rgb(
    bytes: &[u8],
    limits: &ViewImageLimits,
    rotation: u16,
) -> Result<DecodedImage, ToolError> {
    if !matches!(rotation, 0 | 90 | 180 | 270) {
        return Err(ToolError::InvalidInput(
            "rotation_degrees must be 0, 90, 180 or 270".into(),
        ));
    }
    let (mime, mut decoder) = prepare::decoder(bytes, limits)?;
    let (original_width, original_height) = decoder.dimensions();
    if u64::from(original_width) * u64::from(original_height) * 4 > limits.max_decode_bytes as u64 {
        return Err(ToolError::LimitExceeded(
            "RGB conversion exceeds max_decode_bytes".into(),
        ));
    }
    let orientation = decoder.orientation().map_err(prepare::image_error)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(prepare::image_error)?;
    image.apply_orientation(orientation);
    image = match rotation {
        90 => image.rotate90(),
        180 => image.rotate180(),
        270 => image.rotate270(),
        _ => image,
    };
    let width = image.width();
    let height = image.height();
    let pixels = if image.color().has_alpha() {
        image
            .into_rgba8()
            .pixels()
            .flat_map(|pixel| {
                let alpha = u16::from(pixel.0[3]);
                [pixel.0[0], pixel.0[1], pixel.0[2]].map(|value| {
                    // The weighted sum is within 0..=65025; division fits a byte.
                    u8::try_from((u16::from(value) * alpha + 255 * (255 - alpha) + 127) / 255)
                        .unwrap_or(255)
                })
            })
            .collect()
    } else {
        image.into_rgb8().into_raw()
    };
    Ok(DecodedImage {
        mime,
        original_width,
        original_height,
        orientation_applied: orientation != Orientation::NoTransforms,
        width,
        height,
        pixels,
    })
}
