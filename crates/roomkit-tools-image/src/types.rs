use std::fmt;
use std::num::NonZeroU64;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An image to view.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewImageInput {
    /// PNG, JPEG or static WebP file in the workspace.
    pub path: String,
    /// Deadline in ms, at most the host maximum.
    pub timeout_ms: Option<NonZeroU64>,
}

impl ViewImageInput {
    /// View `path` within the host's default deadline, as Python's `view(path)`.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            timeout_ms: None,
        }
    }
}

/// Supported detected media types, independent of the file extension.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum ImageMime {
    /// Portable Network Graphics (static only).
    #[serde(rename = "image/png")]
    Png,
    /// JPEG.
    #[serde(rename = "image/jpeg")]
    Jpeg,
    /// WebP (static only).
    #[serde(rename = "image/webp")]
    Webp,
}

impl ImageMime {
    /// The MIME string used on the wire. Python reads it from the JSON; this
    /// helper is for sibling crates, hidden from the public API.
    #[doc(hidden)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// Image bytes for a host's multimodal result, never ordinary model prose.
#[derive(Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ImagePayload {
    /// Encoded media type; currently always PNG.
    pub mime_type: ImageMime,
    /// Standard base64 of the complete encoded preview. Never truncated.
    pub base64: String,
}

impl ImagePayload {
    /// Return a data URL for an image-capable provider or adapter. Python
    /// builds it from the JSON; hidden from the public API.
    #[doc(hidden)]
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime_type.as_str(), self.base64)
    }
}

impl fmt::Debug for ImagePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImagePayload")
            .field("mime_type", &self.mime_type)
            .field("base64_chars", &self.base64.len())
            .finish_non_exhaustive()
    }
}

/// Metadata and complete pixels. The source file is never modified.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ViewImageResult {
    /// Workspace-relative source path, usable by the other tools.
    pub path: String,
    /// SHA-256 revision of original file bytes, before any transformation.
    pub revision: String,
    /// Detected original media type.
    pub source_mime_type: ImageMime,
    /// Original encoded file size.
    pub input_bytes: usize,
    /// Source width before orientation correction.
    pub original_width: u32,
    /// Source height before orientation correction.
    pub original_height: u32,
    /// Prepared image width.
    pub width: u32,
    /// Prepared image height.
    pub height: u32,
    /// Whether decoded orientation metadata required a transform.
    pub orientation_applied: bool,
    /// Whether the oriented image was reduced to fit the host's maximum edge.
    pub resized: bool,
    /// Encoded preview size before base64 expansion.
    pub output_bytes: usize,
    /// Complete PNG preview, re-encoded as 8-bit pixels without source metadata.
    pub image: ImagePayload,
}

/// Host-owned resource limits. Decoder allocation limits are best effort, not
/// a total heap quota. Source pixels and dimensions are checked before decode.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewImageLimits {
    /// Maximum source file bytes (default 10 MiB).
    pub max_file_bytes: usize,
    /// Maximum source width or height (default 16,384).
    pub max_dimension: usize,
    /// Maximum source pixel count (default 16,777,216).
    pub max_pixels: usize,
    /// Best-effort decoder allocation bound (default 128 MiB). Orientation and
    /// resizing allocate beyond it: a call can peak near 2.25 times this value.
    pub max_decode_bytes: usize,
    /// Maximum preview width or height (default 2,048); no upscaling.
    pub max_edge: usize,
    /// Maximum encoded PNG bytes, excluding base64/metadata (default 4 MiB).
    pub max_image_bytes: usize,
    /// Default total deadline including queueing and I/O (default 10 seconds).
    pub default_timeout_ms: usize,
    /// Maximum allowed per-call deadline (default 120 seconds).
    pub max_timeout_ms: usize,
}

impl Default for ViewImageLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 10 * 1024 * 1024,
            max_dimension: 16_384,
            max_pixels: 16_777_216,
            max_decode_bytes: 128 * 1024 * 1024,
            max_edge: 2048,
            max_image_bytes: 4 * 1024 * 1024,
            default_timeout_ms: 10_000,
            max_timeout_ms: 120_000,
        }
    }
}
