//! Optional RustO/RTen backend. No downloads, Python, RoomKit or global pool changes.
//! Model files are trusted host configuration and must remain unchanged during load.
//! Load on a blocking worker, then share the resulting engine across OCR tools.

mod session;

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use roomkit_tools_core::Error;
use roomkit_tools_image::DecodedImage;
use roomkit_tools_ocr::{OcrBackend, OcrBlock, OcrEngine, OcrIdentity, OcrPoint};
use rusto::{DetectTextResult, InitializeConfig, OcrRunOptions, RustO};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// Explicit PP-OCRv6 model paths and host execution budgets.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RustoConfig {
    /// Trusted ONNX or `RTen` detector model.
    pub detection_model: PathBuf,
    /// Trusted ONNX or `RTen` recognizer model.
    pub recognition_model: PathBuf,
    /// Matching UTF-8 recognizer vocabulary.
    pub dictionary: PathBuf,
    /// Host label, for example `ppocrv6-tiny`.
    pub model_name: String,
    /// Threads per dedicated inference/preprocessing pool, 1..=16.
    #[serde(default = "default_threads")]
    pub threads: usize,
    /// Maximum detector input edge, 32..=2048; avoids default short-edge upscaling.
    #[serde(default = "default_edge")]
    pub detection_max_edge: usize,
}

const fn default_threads() -> usize {
    2
}
const fn default_edge() -> usize {
    960
}

/// Loaded engine implementation. Its pools are private to this backend.
pub struct RustoBackend {
    engine: RustO,
    pool: rayon::ThreadPool,
    identity: OcrIdentity,
    options: OcrRunOptions,
}

impl std::fmt::Debug for RustoBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RustoBackend")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl RustoBackend {
    /// Load local PP-OCRv6 models once. Each model is capped at 256 MiB, dictionary at 4 MiB.
    /// This synchronous initialization belongs on a host-owned blocking worker.
    ///
    /// # Errors
    /// Rejects invalid configuration, unreadable/oversized models and initialization failures.
    pub fn load(config: RustoConfig) -> Result<OcrEngine, Error> {
        OcrEngine::new(Self::load_backend(config)?)
    }

    /// Defer loading until the first OCR call's owned worker. Failed loads are retryable.
    ///
    /// # Errors
    /// Rejects invalid configuration before creating the engine.
    pub fn lazy(config: RustoConfig) -> Result<OcrEngine, Error> {
        validate(&config)?;
        OcrEngine::new(LazyBackend {
            config,
            loaded: None,
        })
    }

    fn load_backend(config: RustoConfig) -> Result<Self, Error> {
        validate(&config)?;
        let detection = read_file(&config.detection_model, 256 * 1024 * 1024)?;
        let recognition = read_file(&config.recognition_model, 256 * 1024 * 1024)?;
        let dictionary = read_file(&config.dictionary, 4 * 1024 * 1024)?;
        std::str::from_utf8(&dictionary)
            .map_err(|_| Error::InvalidInput("OCR dictionary must be UTF-8".into()))?;
        let mut digest = Sha256::new();
        for bytes in [&detection, &recognition, &dictionary] {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        let pool = Arc::new(rten::ThreadPool::with_num_threads(config.threads));
        let det = session::Session {
            model: rten::Model::load(detection).map_err(|_| model_error())?,
            pool: Arc::clone(&pool),
        };
        let rec = session::Session {
            model: rten::Model::load(recognition).map_err(|_| model_error())?,
            pool,
        };
        let preset = InitializeConfig::ppv6(
            config.detection_model,
            config.recognition_model,
            config.dictionary,
        );
        let engine = RustO::with_custom_engines(preset, det, rec).map_err(|_| model_error())?;
        let options = OcrRunOptions {
            detection: Some(rusto::rusto_ocr::DetectionRunOptions {
                limit_side_len: Some(
                    i32::try_from(config.detection_max_edge).map_err(|_| model_error())?,
                ),
                limit_type: Some("max".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.threads)
            .build()
            .map_err(|_| model_error())?;
        Ok(Self {
            engine,
            pool,
            options,
            identity: OcrIdentity {
                provider: "rusto".into(),
                model: config.model_name,
                revision: Some(format!("sha256:{:x}", digest.finalize())),
            },
        })
    }
}

fn validate(config: &RustoConfig) -> Result<(), Error> {
    if !(1..=16).contains(&config.threads)
        || !(32..=2048).contains(&config.detection_max_edge)
        || config.model_name.is_empty()
        || config.model_name.len() > 256
    {
        return Err(Error::InvalidInput(
            "invalid RustO model label or worker configuration".into(),
        ));
    }
    Ok(())
}

struct LazyBackend {
    config: RustoConfig,
    loaded: Option<RustoBackend>,
}

impl OcrBackend for LazyBackend {
    fn identity(&self) -> OcrIdentity {
        self.loaded.as_ref().map_or_else(
            || OcrIdentity {
                provider: "rusto".into(),
                model: self.config.model_name.clone(),
                revision: None,
            },
            OcrBackend::identity,
        )
    }
    fn recognize(
        &mut self,
        image: &DecodedImage,
        cancel: &CancellationToken,
    ) -> Result<Vec<OcrBlock>, Error> {
        if self.loaded.is_none() {
            self.loaded = Some(RustoBackend::load_backend(self.config.clone())?);
        }
        self.loaded
            .as_mut()
            .ok_or_else(model_error)?
            .recognize(image, cancel)
    }
}

impl OcrBackend for RustoBackend {
    fn identity(&self) -> OcrIdentity {
        self.identity.clone()
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "tool image dimensions are bounded; f32 is the upstream polygon coordinate type"
    )]
    fn recognize(
        &mut self,
        image: &DecodedImage,
        cancel: &CancellationToken,
    ) -> Result<Vec<OcrBlock>, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let stride = image.width as usize * 3;
        let result = self
            .pool
            .install(|| {
                self.engine.detect_text_rgb(
                    &image.pixels,
                    image.width,
                    image.height,
                    stride,
                    &self.options,
                )
            })
            .map_err(|_| {
                Error::Unsupported("RustO inference failed for the configured models/image".into())
            })?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let DetectTextResult::Structured(items) = result else {
            return Err(Error::Unsupported(
                "RustO returned an unexpected output format".into(),
            ));
        };
        Ok(items
            .into_iter()
            .map(|item| OcrBlock {
                text: item.text,
                score: Some(item.score),
                // Detector expansions can extend just beyond an image boundary.
                polygon: item.box_points.map(|(x, y)| OcrPoint {
                    x: x.clamp(0.0, image.width as f32),
                    y: y.clamp(0.0, image.height as f32),
                }),
            })
            .collect())
    }
}

fn model_error() -> Error {
    Error::Unsupported("unable to initialize the configured RustO models/pools".into())
}

fn read_file(path: &Path, maximum: u64) -> Result<Vec<u8>, Error> {
    let meta = std::fs::metadata(path).map_err(|source| Error::Io {
        operation: "inspect OCR model path",
        source,
    })?;
    if !meta.is_file() {
        return Err(Error::InvalidInput(
            "OCR models must be regular files".into(),
        ));
    }

    let file = File::open(path).map_err(|source| Error::Io {
        operation: "open OCR model file",
        source,
    })?;
    let meta = file.metadata().map_err(|source| Error::Io {
        operation: "inspect OCR model file",
        source,
    })?;
    if !meta.is_file() {
        return Err(Error::InvalidInput(
            "OCR models must be regular files".into(),
        ));
    }
    if meta.len() > maximum {
        return Err(Error::LimitExceeded("OCR model file is too large".into()));
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::Io {
            operation: "read OCR model file",
            source,
        })?;
    if bytes.len() as u64 > maximum {
        return Err(Error::LimitExceeded("OCR model file is too large".into()));
    }
    Ok(bytes)
}
