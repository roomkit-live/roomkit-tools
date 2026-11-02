use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One local image to transcribe. Host chooses the engine and models.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OcrInput {
    /// Regular static PNG/JPEG/WebP beneath the workspace root.
    pub path: String,
    /// Extra clockwise rotation after EXIF; 0, 90, 180 or 270. No auto orientation.
    #[serde(default)]
    pub rotation_degrees: u16,
    /// Optional raw-byte SHA-256 revision to reject a changed image.
    pub expected_revision: Option<String>,
    /// Positive timeout capped by the host. Native stages may finish after return.
    pub timeout_ms: Option<usize>,
}

impl OcrInput {
    /// Transcribe `path` as stored, with no revision check and the host's
    /// default timeout, as Python's `ocr(path)`.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            rotation_degrees: 0,
            expected_revision: None,
            timeout_ms: None,
        }
    }
}

/// Host resource bounds, independent of the selected recognition engine.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OcrLimits {
    /// Encoded source bytes.
    pub max_file_bytes: usize,
    /// Maximum source width/height.
    pub max_dimension: usize,
    /// Maximum decoded source pixels.
    pub max_pixels: usize,
    /// Decoder/conversion allocation bound, not total native heap.
    pub max_decode_bytes: usize,
    /// Complete serialized Output envelope budget.
    pub max_output_bytes: usize,
    /// Maximum complete returned blocks.
    pub max_blocks: usize,
    /// Default deadline including queueing and decoding.
    pub default_timeout_ms: usize,
    /// Maximum allowed deadline.
    pub max_timeout_ms: usize,
}

impl Default for OcrLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_dimension: 8192,
            max_pixels: 16_000_000,
            max_decode_bytes: 128 * 1024 * 1024,
            max_output_bytes: 64 * 1024,
            max_blocks: 256,
            default_timeout_ms: 30_000,
            max_timeout_ms: 120_000,
        }
    }
}

/// Host-selected backend and model identity, never inferred from OCR content.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OcrIdentity {
    /// Backend identifier.
    pub provider: String,
    /// Human-readable model selection.
    pub model: String,
    /// Backend-specific immutable model fingerprint when available.
    pub revision: Option<String>,
}

/// Pixel coordinate in the oriented image, origin at its top-left.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OcrPoint {
    /// Horizontal pixel position.
    pub x: f32,
    /// Vertical pixel position.
    pub y: f32,
}

/// Recognized text and its geometry; text can contain recognition errors.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OcrBlock {
    /// Complete recognized block, without fabricated layout whitespace.
    pub text: String,
    /// Engine score in 0..=1, not calibrated probability of correctness.
    pub score: Option<f32>,
    /// Four polygon corners in the oriented image's pixel coordinates.
    pub polygon: [OcrPoint; 4],
}

/// Bounded transcription with source identity and explicit image transforms.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OcrResult {
    /// Workspace-relative source path, usable by the other tools.
    pub path: String,
    /// SHA-256 of the original encoded bytes, before transforms.
    pub revision: String,
    /// Backend/model used for this result.
    pub backend: OcrIdentity,
    /// Original source width before transforms.
    pub original_width: u32,
    /// Original source height before transforms.
    pub original_height: u32,
    /// Width of the coordinate space used by all blocks.
    pub width: u32,
    /// Height of the coordinate space used by all blocks.
    pub height: u32,
    /// Whether EXIF orientation changed the image.
    pub orientation_applied: bool,
    /// Extra clockwise rotation after EXIF, as requested.
    pub rotation_degrees: u16,
    /// Returned block texts joined by newlines; not exact source formatting.
    pub text: String,
    /// Complete blocks in backend reading order.
    pub blocks: Vec<OcrBlock>,
    /// Whether output bounds omitted blocks.
    pub truncated: bool,
    /// Number of complete recognized blocks omitted from this response.
    pub omitted_blocks: usize,
}
