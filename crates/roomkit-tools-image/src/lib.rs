//! Local image preparation for vision-capable agents, independent of providers.
//!
//! Accepts static PNG/JPEG/WebP, decodes with host bounds and returns a complete
//! normalized PNG preview plus metadata. Hosts must deliver the image payload as
//! multimodal content; serializing it into ordinary text does not enable vision.

#![forbid(unsafe_code)]

mod decoded;
mod prepare;
mod source;
mod types;
pub use decoded::{DecodedImage, ImageWorkspace, decode_rgb};

pub use types::{ImageMime, ImagePayload, ViewImageInput, ViewImageLimits, ViewImageResult};

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use roomkit_tools_core::{Definition, Output, ToolError, WorkspaceRoot};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Read-only image preparation under a canonical host-selected root.
///
/// Clones share two worker slots, retained through decoding even when the caller
/// times out or cancels. Decoder/resize operations cannot be preempted mid-stage;
/// cancellation is checked between stages and while writing encoded chunks.
/// Root checks assume a host-controlled filesystem, not hostile path replacement.
#[derive(Clone, Debug)]
pub struct ViewImageTool {
    root: WorkspaceRoot,
    limits: ViewImageLimits,
    workers: Arc<Semaphore>,
}

impl ViewImageTool {
    /// Resolve a directory root asynchronously without changing the working directory.
    ///
    /// # Errors
    /// Returns I/O errors for inaccessible paths and invalid input for a file root.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: source::root(root.as_ref()).await?,
            limits: ViewImageLimits::default(),
            workers: Arc::new(Semaphore::new(2)),
        })
    }

    /// Set positive, representable host-owned limits.
    ///
    /// # Errors
    /// Refuses invalid bounds and deadlines over 24 hours.
    pub fn with_limits(mut self, limits: ViewImageLimits) -> Result<Self, ToolError> {
        if [
            limits.max_file_bytes,
            limits.max_dimension,
            limits.max_pixels,
            limits.max_decode_bytes,
            limits.max_edge,
            limits.max_image_bytes,
            limits.default_timeout_ms,
            limits.max_timeout_ms,
        ]
        .contains(&0)
            || limits.default_timeout_ms > limits.max_timeout_ms
            || limits.max_timeout_ms > 86_400_000
            || u32::try_from(limits.max_dimension).is_err()
            || u32::try_from(limits.max_edge).is_err()
            || u64::try_from(limits.max_file_bytes).map_or(true, |n| n == u64::MAX)
            || limits
                .max_image_bytes
                .checked_add(2)
                .and_then(|n| (n / 3).checked_mul(4))
                .is_none()
        {
            return Err(ToolError::InvalidInput(
                "image limits must be positive and representable; deadlines at most 24 hours"
                    .into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// Input and output schemas derived from the actual Rust types.
    pub fn definition() -> Definition {
        Definition::new::<ViewImageInput, ViewImageResult>(
            "view_image",
            "View one local static PNG, JPEG or WebP file. Returns actual image pixels for vision input with source dimensions, revision and preview transformation metadata. Use for screenshots, diagrams and photos; read is for text. The source is not modified. Large images may be downscaled by host policy; inspect resized and dimensions. Requires a vision-capable model and multimodal output transport. No OCR, remote URLs, PDF, SVG or animations.",
        )
    }

    /// Read and prepare one image, returning a complete PNG payload and short text.
    ///
    /// # Errors
    /// Distinguishes invalid input, containment refusals, I/O failures, invalid or
    /// unsupported images, exceeded budgets and cancellation. No partial image is returned.
    pub async fn view(
        &self,
        input: ViewImageInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ViewImageResult>, ToolError> {
        if input.path.is_empty() || input.path.len() > 4096 || input.path.contains('\0') {
            return Err(ToolError::InvalidInput(
                "path must be nonempty, NUL-free and at most 4096 bytes".into(),
            ));
        }
        let timeout = input.timeout_ms.unwrap_or(self.limits.default_timeout_ms);
        if timeout == 0 || timeout > self.limits.max_timeout_ms {
            return Err(ToolError::InvalidInput(
                "timeout_ms must be positive and within the host maximum".into(),
            ));
        }
        let duration = Duration::from_millis(timeout as u64);
        let token = cancel.child_token();
        let _guard = token.clone().drop_guard();
        let deadline = Instant::now() + duration;
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ToolError::Cancelled),
            result = tokio::time::timeout(duration, self.run(input, token.clone(), deadline)) =>
                result.map_err(|_| ToolError::LimitExceeded("view_image deadline exceeded".into()))?,
        }
    }

    async fn run(
        &self,
        input: ViewImageInput,
        token: CancellationToken,
        deadline: Instant,
    ) -> Result<Output<ViewImageResult>, ToolError> {
        let permit = Arc::clone(&self.workers)
            .acquire_owned()
            .await
            .map_err(|_| ToolError::Cancelled)?;
        let (path, bytes) =
            source::load(&self.root, &input.path, self.limits.max_file_bytes).await?;
        let path = self.root.relative(&path)?;
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            prepare::prepare(path, &bytes, limits, &prepare::Control { token, deadline })
        })
        .await
        .map_err(|_| ToolError::UnsupportedResource("image worker failed".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queued_work_obeys_deadline_and_cancellation_before_file_io() {
        let root = tempfile::tempdir().unwrap();
        let tool = ViewImageTool::new(root.path()).await.unwrap();
        let _slots = tool.workers.acquire_many(2).await.unwrap();
        let input = ViewImageInput {
            path: "missing.png".into(),
            timeout_ms: Some(10),
        };
        assert!(matches!(
            tool.view(input, &CancellationToken::new()).await,
            Err(ToolError::LimitExceeded(_))
        ));
        let cancel = CancellationToken::new();
        let mut call = Box::pin(tool.view(ViewImageInput::new("missing.png"), &cancel));
        tokio::select! {
            biased;
            _result = &mut call => panic!("queued view must not finish yet"),
            () = tokio::task::yield_now() => (),
        }
        cancel.cancel();
        assert!(matches!(call.await, Err(ToolError::Cancelled)));
    }
}
