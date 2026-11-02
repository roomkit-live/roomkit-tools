//! Optional RustO/RTen backend. No downloads, Python, RoomKit or global pool changes.
//! Model files are trusted host configuration and must remain unchanged during load.
//! Load on a blocking worker, then share the resulting engine across OCR tools.

#![forbid(unsafe_code)]

mod session;

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use roomkit_tools_core::ToolError;
use roomkit_tools_image::DecodedImage;
use roomkit_tools_ocr::{OcrBackend, OcrBlock, OcrEngine, OcrIdentity, OcrPoint};
use rusto::{DetectTextResult, InferenceSession, InitializeConfig, OcrRunOptions, RustO};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// A PP-OCRv6 models directory and host execution budgets, Python's
/// `RustoOcr(models, *, model_name, threads, detection_max_edge)`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RustoConfig {
    /// Trusted directory holding the ONNX or `RTen` detector model `det.onnx`,
    /// the recognizer model `rec.onnx` and its UTF-8 vocabulary `dict.txt`.
    pub models: PathBuf,
    /// Host label, for example `ppocrv6-tiny`. Default: `ppocrv6`.
    #[serde(default = "default_model_name")]
    pub model_name: String,
    /// Threads per dedicated inference/preprocessing pool, 1..=16. Default: 2.
    #[serde(default = "default_threads")]
    pub threads: usize,
    /// Maximum detector input edge, 32..=2048; avoids default short-edge
    /// upscaling. Default: 960.
    #[serde(default = "default_edge")]
    pub detection_max_edge: usize,
}

impl RustoConfig {
    /// The models in `models`, with the default label, threads and edge.
    pub fn new(models: impl Into<PathBuf>) -> Self {
        Self {
            models: models.into(),
            model_name: default_model_name(),
            threads: default_threads(),
            detection_max_edge: default_edge(),
        }
    }

    fn detection_model(&self) -> PathBuf {
        self.models.join("det.onnx")
    }

    fn recognition_model(&self) -> PathBuf {
        self.models.join("rec.onnx")
    }

    fn dictionary(&self) -> PathBuf {
        self.models.join("dict.txt")
    }
}

fn default_model_name() -> String {
    "ppocrv6".into()
}
const fn default_threads() -> usize {
    2
}
const fn default_edge() -> usize {
    960
}

/// Loaded engine implementation. Its pools are private to this backend.
pub struct RustoOcr {
    engine: RustO,
    pool: rayon::ThreadPool,
    identity: OcrIdentity,
    options: OcrRunOptions,
}

impl std::fmt::Debug for RustoOcr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RustoOcr")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl RustoOcr {
    /// Load local PP-OCRv6 models once. Each model is capped at 256 MiB, dictionary at 4 MiB.
    /// Each file is read once; the engine uses the bytes that were checked and fingerprinted.
    /// This synchronous initialization belongs on a host-owned blocking worker.
    ///
    /// # Errors
    /// Rejects invalid configuration, unreadable/oversized models and initialization failures.
    pub fn load(config: RustoConfig) -> Result<OcrEngine, ToolError> {
        OcrEngine::new(Self::load_backend(config)?)
    }

    /// Defer loading until the first OCR call's owned worker. Failed loads are retryable.
    ///
    /// # Errors
    /// Rejects invalid configuration before creating the engine.
    pub fn lazy(config: RustoConfig) -> Result<OcrEngine, ToolError> {
        validate(&config)?;
        OcrEngine::new(LazyBackend {
            config,
            loaded: None,
        })
    }

    fn load_backend(config: RustoConfig) -> Result<Self, ToolError> {
        validate(&config)?;
        let detection = read_file(&config.detection_model(), 256 * 1024 * 1024)?;
        let recognition = read_file(&config.recognition_model(), 256 * 1024 * 1024)?;
        let dictionary = read_file(&config.dictionary(), 4 * 1024 * 1024)?;
        let characters = vocabulary(
            std::str::from_utf8(&dictionary)
                .map_err(|_| ToolError::InvalidInput("OCR dictionary must be UTF-8".into()))?,
        );
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
        let engine = engine(&config, det, rec, characters)?;
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
                revision: Some(roomkit_tools_core::revision::sha256(&digest.finalize())),
            },
        })
    }
}

/// Split the validated dictionary into entries exactly as `RustO` reads a
/// dictionary file: one entry per line, `\n` or `\r\n` terminated.
fn vocabulary(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// Build the engine from loaded sessions and the validated dictionary entries.
/// No dictionary path is passed on, so the engine cannot read the file again.
fn engine(
    config: &RustoConfig,
    det: impl InferenceSession + 'static,
    rec: impl InferenceSession + 'static,
    characters: Vec<String>,
) -> Result<RustO, ToolError> {
    let mut preset = InitializeConfig::ppv6(
        config.detection_model(),
        config.recognition_model(),
        config.dictionary(),
    );
    preset.rec.rec_keys_path = None;
    let rec = session::Vocabulary {
        session: rec,
        characters,
    };
    RustO::with_custom_engines(preset, det, rec).map_err(|_| model_error())
}

fn validate(config: &RustoConfig) -> Result<(), ToolError> {
    if !(1..=16).contains(&config.threads)
        || !(32..=2048).contains(&config.detection_max_edge)
        || config.model_name.is_empty()
        || config.model_name.len() > 256
    {
        return Err(ToolError::InvalidInput(
            "invalid RustO model label or worker configuration".into(),
        ));
    }
    Ok(())
}

struct LazyBackend {
    config: RustoConfig,
    loaded: Option<RustoOcr>,
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
    ) -> Result<Vec<OcrBlock>, ToolError> {
        if self.loaded.is_none() {
            self.loaded = Some(RustoOcr::load_backend(self.config.clone())?);
        }
        self.loaded
            .as_mut()
            .ok_or_else(model_error)?
            .recognize(image, cancel)
    }
}

impl OcrBackend for RustoOcr {
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
    ) -> Result<Vec<OcrBlock>, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
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
                ToolError::UnsupportedResource(
                    "RustO inference failed for the configured models/image".into(),
                )
            })?;
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let DetectTextResult::Structured(items) = result else {
            return Err(ToolError::UnsupportedResource(
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

fn model_error() -> ToolError {
    ToolError::UnsupportedResource("unable to initialize the configured RustO models/pools".into())
}

fn read_file(path: &Path, maximum: u64) -> Result<Vec<u8>, ToolError> {
    let meta = std::fs::metadata(path).map_err(|source| ToolError::Io {
        operation: "inspect OCR model path",
        source,
    })?;
    if !meta.is_file() {
        return Err(ToolError::InvalidInput(
            "OCR models must be regular files".into(),
        ));
    }

    let file = File::open(path).map_err(|source| ToolError::Io {
        operation: "open OCR model file",
        source,
    })?;
    let meta = file.metadata().map_err(|source| ToolError::Io {
        operation: "inspect OCR model file",
        source,
    })?;
    if !meta.is_file() {
        return Err(ToolError::InvalidInput(
            "OCR models must be regular files".into(),
        ));
    }
    if meta.len() > maximum {
        return Err(ToolError::LimitExceeded(
            "OCR model file is too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| ToolError::Io {
            operation: "read OCR model file",
            source,
        })?;
    if bytes.len() as u64 > maximum {
        return Err(ToolError::LimitExceeded(
            "OCR model file is too large".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{ArrayD, IxDyn};
    use rusto::engine::EngineError;
    use rusto::image_impl::Mat;
    use std::collections::HashMap;

    /// Inference stub returning fixed recognizer scores.
    struct Scores(ArrayD<f32>);

    impl InferenceSession for Scores {
        fn run(&mut self, _input: ArrayD<f32>) -> Result<ArrayD<f32>, EngineError> {
            Ok(self.0.clone())
        }
        fn run_all(
            &mut self,
            _input: ArrayD<f32>,
        ) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
            Err(EngineError::OutputError("not used".into()))
        }
        fn run_with_inputs(
            &mut self,
            _inputs: HashMap<String, ArrayD<f32>>,
        ) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
            Err(EngineError::OutputError("not used".into()))
        }
    }

    fn config(models: &Path) -> RustoConfig {
        RustoConfig {
            model_name: "test".into(),
            threads: 1,
            ..RustoConfig::new(models)
        }
    }

    /// Scores selecting vocabulary entries 1 and 2, then the CTC blank.
    fn scores_for_first_two_entries() -> ArrayD<f32> {
        let mut scores = ArrayD::zeros(IxDyn(&[1, 3, 4]));
        scores[[0, 0, 1]] = 1.0;
        scores[[0, 1, 2]] = 1.0;
        scores[[0, 2, 0]] = 1.0;
        scores
    }

    #[test]
    fn recognizer_decodes_with_the_validated_dictionary_bytes() {
        // The configured file holds other content than the validated bytes, as
        // when the file changes after validation: decoding must not reread it.
        let models = tempfile::tempdir().unwrap();
        std::fs::write(models.path().join("dict.txt"), "x\ny\nz\n").unwrap();
        let validated = std::str::from_utf8(b"a\nb\n").unwrap();
        let mut engine = engine(
            &config(models.path()),
            Scores(ArrayD::zeros(IxDyn(&[1]))),
            Scores(scores_for_first_two_entries()),
            vocabulary(validated),
        )
        .unwrap();
        let line = Mat::from_rgb8(8, 8, vec![255; 8 * 8 * 3]).unwrap();
        let output = engine.rec.run(&[line], false).unwrap();
        assert_eq!(output.txts, ["ab"]);
    }

    #[test]
    fn vocabulary_matches_the_entries_rusto_reads_from_a_file() {
        let directory = tempfile::tempdir().unwrap();
        for text in [
            "",
            "a",
            "a\nb\n",
            "a\r\nb\r\n\nc",
            "é\n\n \nlast\r",
            "x\ry\n",
        ] {
            let path = directory.path().join("dict.txt");
            std::fs::write(&path, text).unwrap();
            let decoder = rusto::rec::CtcDecoder::from_file(&path).unwrap();
            let mut expected = vec!["blank".to_owned()];
            expected.extend(vocabulary(text));
            expected.push(" ".to_owned());
            assert_eq!(decoder.chars, expected, "{text:?}");
        }
    }
}

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
