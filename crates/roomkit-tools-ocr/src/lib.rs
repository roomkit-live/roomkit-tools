//! Local OCR contracts and bounded execution, independent of recognition engines.
//!
//! Share an [`OcrEngine`] across tools to reuse models and serialize native work.
//! Cancellation releases the caller promptly; the worker and its permit remain
//! owned until decoding/inference finishes. Inputs assume a host-controlled tree.

#![forbid(unsafe_code)]

mod types;
pub use types::{OcrBlock, OcrIdentity, OcrInput, OcrLimits, OcrPoint, OcrResult};

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use roomkit_tools_core::{Definition, Error, Output};
pub use roomkit_tools_image::DecodedImage;
use roomkit_tools_image::{ImageWorkspace, ViewImageLimits, decode_rgb};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Implementations run synchronously inside an owned blocking worker.
/// Backends must return finite coordinates in the supplied oriented image and
/// must not silently rotate, modify the source or fetch remote resources.
pub trait OcrBackend: Send + 'static {
    /// Stable host-selected identity for result provenance.
    fn identity(&self) -> OcrIdentity;
    /// Recognize complete blocks in reading order. Observe cancellation where possible.
    ///
    /// # Errors
    /// Returns backend configuration, recognition or cancellation errors.
    fn recognize(
        &mut self,
        image: &DecodedImage,
        cancel: &CancellationToken,
    ) -> Result<Vec<OcrBlock>, Error>;
}

/// Shared model lifetime and a single in-flight decode/inference slot.
#[derive(Clone)]
pub struct OcrEngine {
    backend: Arc<Mutex<Box<dyn OcrBackend>>>,
    identity: OcrIdentity,
    slot: Arc<Semaphore>,
}

impl std::fmt::Debug for OcrEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OcrEngine")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl OcrEngine {
    /// Own a backend. Clones share model state and the execution slot.
    ///
    /// # Errors
    /// Rejects empty or oversized backend identity fields.
    pub fn new(backend: impl OcrBackend) -> Result<Self, Error> {
        let identity = backend.identity();
        if identity.provider.is_empty()
            || identity.provider.len() > 128
            || identity.model.is_empty()
            || identity.model.len() > 256
            || identity.revision.as_ref().is_some_and(|s| s.len() > 256)
        {
            return Err(Error::InvalidInput("invalid OCR backend identity".into()));
        }
        Ok(Self {
            backend: Arc::new(Mutex::new(Box::new(backend))),
            identity,
            slot: Arc::new(Semaphore::new(1)),
        })
    }
}

/// Individually selectable OCR under an explicit workspace and limits.
#[derive(Clone, Debug)]
pub struct OcrTool {
    workspace: ImageWorkspace,
    engine: OcrEngine,
    limits: OcrLimits,
}

impl OcrTool {
    /// Resolve a workspace and share a host-created recognition engine.
    ///
    /// # Errors
    /// Returns an error if the workspace is not an accessible directory.
    pub async fn new(root: impl AsRef<Path>, engine: OcrEngine) -> Result<Self, Error> {
        Ok(Self {
            workspace: ImageWorkspace::new(root).await?,
            engine,
            limits: OcrLimits::default(),
        })
    }

    /// Set host bounds. These do not constitute a process-wide native heap quota.
    ///
    /// # Errors
    /// Rejects zero, inconsistent or unrepresentable bounds.
    pub fn with_limits(mut self, limits: OcrLimits) -> Result<Self, Error> {
        if [
            limits.max_file_bytes,
            limits.max_dimension,
            limits.max_pixels,
            limits.max_decode_bytes,
            limits.max_output_bytes,
            limits.max_blocks,
            limits.default_timeout_ms,
            limits.max_timeout_ms,
        ]
        .contains(&0)
            || limits.max_file_bytes == usize::MAX
            || limits.max_dimension > u32::MAX as usize
            || limits.default_timeout_ms > limits.max_timeout_ms
            || limits.max_timeout_ms > 86_400_000
        {
            return Err(Error::InvalidInput("invalid OCR resource limits".into()));
        }
        self.limits = limits;
        Ok(self)
    }

    /// Schemas derived from the actual Rust input and output types.
    pub fn definition() -> Definition {
        Definition::new::<OcrInput, OcrResult>(
            "ocr",
            "Extract text and positioned blocks from one local static PNG/JPEG/WebP. Host selects the OCR engine. Recognition may contain errors; scores are not proof of correctness. Coordinates refer to width/height after EXIF and requested clockwise rotation. Use rotation_degrees for turned images (0/90/180/270); no automatic orientation. Inspect truncated/omitted_blocks. Does not modify files, interpret diagrams or read PDFs.",
        )
    }

    /// Transcribe an image. Deadline includes queueing, loading and native work.
    ///
    /// # Errors
    /// Returns input, path, revision, decode, backend, limit or cancellation errors.
    pub async fn ocr(
        &self,
        input: OcrInput,
        cancel: &CancellationToken,
    ) -> Result<Output<OcrResult>, Error> {
        if !matches!(input.rotation_degrees, 0 | 90 | 180 | 270) {
            return Err(Error::InvalidInput(
                "rotation_degrees must be 0, 90, 180 or 270".into(),
            ));
        }
        if let Some(revision) = &input.expected_revision
            && !revision.strip_prefix("sha256:").is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(Error::InvalidInput(
                "expected_revision must be a lowercase sha256 digest".into(),
            ));
        }
        let timeout = input.timeout_ms.unwrap_or(self.limits.default_timeout_ms);
        if timeout == 0 || timeout > self.limits.max_timeout_ms {
            return Err(Error::InvalidInput(
                "timeout_ms is outside the host bounds".into(),
            ));
        }
        let token = cancel.child_token();
        let _guard = token.clone().drop_guard();
        let duration = Duration::from_millis(timeout as u64);
        let deadline = Instant::now() + duration;
        tokio::select! {
            biased;
            () = token.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(duration, self.run(input, token.clone(), deadline)) =>
                result.map_err(|_| Error::LimitExceeded("ocr deadline exceeded".into()))?,
        }
    }

    async fn run(
        &self,
        input: OcrInput,
        token: CancellationToken,
        deadline: Instant,
    ) -> Result<Output<OcrResult>, Error> {
        let permit = Arc::clone(&self.engine.slot)
            .acquire_owned()
            .await
            .map_err(|_| Error::Cancelled)?;
        let (path, bytes) = self
            .workspace
            .load(&input.path, self.limits.max_file_bytes)
            .await?;
        let path = self.workspace.relative(&path)?;
        let engine = self.engine.clone();
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let check = || {
                if token.is_cancelled() {
                    Err(Error::Cancelled)
                } else if Instant::now() >= deadline {
                    Err(Error::LimitExceeded("ocr deadline exceeded".into()))
                } else {
                    Ok(())
                }
            };
            check()?;
            let revision = format!("sha256:{:x}", Sha256::digest(&bytes));
            if input
                .expected_revision
                .as_ref()
                .is_some_and(|expected| expected != &revision)
            {
                return Err(Error::Refused(
                    "image changed since expected_revision".into(),
                ));
            }
            let bounds = ViewImageLimits {
                max_dimension: limits.max_dimension,
                max_pixels: limits.max_pixels,
                max_decode_bytes: limits.max_decode_bytes,
                ..ViewImageLimits::default()
            };
            let image = decode_rgb(&bytes, &bounds, input.rotation_degrees)?;
            check()?;
            let (blocks, identity) = {
                let mut backend = engine.backend.lock().map_err(|_| {
                    Error::Unsupported("OCR backend is unavailable after a worker failure".into())
                })?;
                let blocks = backend.recognize(&image, &token)?;
                (blocks, backend.identity())
            };
            check()?;
            let result = OcrResult {
                path,
                revision,
                backend: identity,
                original_width: image.original_width,
                original_height: image.original_height,
                width: image.width,
                height: image.height,
                orientation_applied: image.orientation_applied,
                rotation_degrees: input.rotation_degrees,
                text: String::new(),
                blocks: Vec::new(),
                truncated: !blocks.is_empty(),
                omitted_blocks: blocks.len(),
            };
            bounded_result(result, blocks, limits, check)
        })
        .await
        .map_err(|_| Error::Unsupported("OCR worker failed".into()))?
    }
}

fn render(result: OcrResult) -> Output<OcrResult> {
    let text = format!(
        "OCR {:?} ({}x{}, rotation={}, truncated={}, omitted_blocks={}):\n{}",
        result.path,
        result.width,
        result.height,
        result.rotation_degrees,
        result.truncated,
        result.omitted_blocks,
        result.text
    );
    Output::new(result, text)
}

fn encoded_len(output: &Output<OcrResult>) -> Result<usize, Error> {
    serde_json::to_vec(output)
        .map(|v| v.len())
        .map_err(|_| Error::Unsupported("invalid OCR output".into()))
}

fn bounded_result(
    mut result: OcrResult,
    blocks: Vec<OcrBlock>,
    limits: OcrLimits,
    check: impl Fn() -> Result<(), Error>,
) -> Result<Output<OcrResult>, Error> {
    if encoded_len(&render(result.clone()))? > limits.max_output_bytes {
        return Err(Error::LimitExceeded(
            "OCR metadata exceeds max_output_bytes".into(),
        ));
    }
    for block in blocks {
        check()?;
        if block
            .score
            .is_some_and(|s| !s.is_finite() || !(0.0..=1.0).contains(&s))
            || block.polygon.iter().any(|p| {
                !p.x.is_finite()
                    || !p.y.is_finite()
                    || p.x < 0.0
                    || p.y < 0.0
                    || f64::from(p.x) > f64::from(result.width)
                    || f64::from(p.y) > f64::from(result.height)
            })
        {
            return Err(Error::Unsupported(
                "OCR backend returned invalid geometry or scores".into(),
            ));
        }
        // Do not copy a single oversized block into multiple envelopes.
        if result.blocks.len() >= limits.max_blocks || block.text.len() > limits.max_output_bytes {
            break;
        }
        result.blocks.push(block);
        result.omitted_blocks -= 1;
        result.truncated = result.omitted_blocks != 0;
        result.text = result
            .blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if encoded_len(&render(result.clone()))? > limits.max_output_bytes {
            result.blocks.pop();
            result.omitted_blocks += 1;
            result.truncated = true;
            result.text = result
                .blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            break;
        }
    }
    check()?;
    Ok(render(result))
}
