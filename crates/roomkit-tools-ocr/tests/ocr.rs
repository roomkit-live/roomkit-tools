//! OCR contract, limits and native-worker ownership checks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test assertions and temporary fixtures"
)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

use roomkit_tools_core::Error;
use roomkit_tools_image::DecodedImage;
use roomkit_tools_ocr::*;
use tokio_util::sync::CancellationToken;

struct Fixture {
    calls: Arc<AtomicUsize>,
    text: String,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<mpsc::Receiver<()>>,
    invalid_score: bool,
}

impl OcrBackend for Fixture {
    fn identity(&self) -> OcrIdentity {
        OcrIdentity {
            provider: "fixture".into(),
            model: "pixels".into(),
            revision: None,
        }
    }
    fn recognize(
        &mut self,
        image: &DecodedImage,
        _: &CancellationToken,
    ) -> Result<Vec<OcrBlock>, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = &self.entered {
            entered.notify_one();
        }
        if let Some(release) = self.release.take() {
            release.recv().unwrap();
        }
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 3
        );
        Ok(vec![
            OcrBlock {
                text: self.text.clone(),
                score: Some(if self.invalid_score { f32::NAN } else { 0.9 }),
                polygon: [OcrPoint { x: 0.0, y: 0.0 }; 4]
            };
            2
        ])
    }
}

fn fixture() -> Fixture {
    Fixture {
        calls: Arc::new(AtomicUsize::new(0)),
        text: "café".into(),
        entered: None,
        release: None,
        invalid_score: false,
    }
}
fn input(path: &str) -> OcrInput {
    serde_json::from_value(serde_json::json!({ "path": path })).unwrap()
}
fn fixture_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    image::RgbImage::from_pixel(3, 2, image::Rgb([255, 255, 255]))
        .save(root.path().join("image.png"))
        .unwrap();
    root
}

#[tokio::test]
async fn geometry_revisions_limits_and_source_integrity() {
    let root = fixture_root();
    let original = std::fs::read(root.path().join("image.png")).unwrap();
    let backend = fixture();
    let calls = Arc::clone(&backend.calls);
    let tool = OcrTool::new(root.path(), OcrEngine::new(backend).unwrap())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let mut request = input("image.png");
    request.rotation_degrees = 90;
    let result = tool.ocr(request.clone(), &cancel).await.unwrap();
    assert_eq!((result.data.width, result.data.height), (2, 3));
    assert_eq!(result.data.text, "café\ncafé");
    assert_eq!(result.data.blocks.len(), 2);
    assert!(!result.data.truncated);
    assert!(result.data.revision.starts_with("sha256:"));
    request.expected_revision = Some(result.data.revision);
    tool.ocr(request.clone(), &cancel).await.unwrap();
    request.expected_revision = Some(format!("sha256:{}", "0".repeat(64)));
    assert!(matches!(
        tool.ocr(request, &cancel).await,
        Err(Error::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let limited = tool
        .clone()
        .with_limits(OcrLimits {
            max_blocks: 1,
            ..Default::default()
        })
        .unwrap();
    let result = limited.ocr(input("image.png"), &cancel).await.unwrap();
    assert_eq!(result.data.omitted_blocks, 1);
    assert!(result.data.truncated);
    assert_eq!(result.data.text, "café");
    let tiny = tool
        .with_limits(OcrLimits {
            max_pixels: 5,
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        tiny.ocr(input("image.png"), &cancel).await,
        Err(Error::LimitExceeded(_))
    ));
    assert_eq!(
        std::fs::read(root.path().join("image.png")).unwrap(),
        original
    );
}

#[tokio::test]
async fn path_input_and_backend_failures_remain_explicit() {
    let root = fixture_root();
    let outside = fixture_root();
    let tool = OcrTool::new(root.path(), OcrEngine::new(fixture()).unwrap())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    assert!(matches!(
        tool.ocr(
            input(outside.path().join("image.png").to_str().unwrap()),
            &cancel
        )
        .await,
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        tool.ocr(input("."), &cancel).await,
        Err(Error::Unsupported(_))
    ));
    std::fs::write(root.path().join("bad.png"), b"bad").unwrap();
    assert!(matches!(
        tool.ocr(input("bad.png"), &cancel).await,
        Err(Error::Unsupported(_))
    ));
    let mut request = input("image.png");
    request.rotation_degrees = 45;
    assert!(matches!(
        tool.ocr(request, &cancel).await,
        Err(Error::InvalidInput(_))
    ));
    let mut invalid = fixture();
    invalid.invalid_score = true;
    let tool = OcrTool::new(root.path(), OcrEngine::new(invalid).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        tool.ocr(input("image.png"), &cancel).await,
        Err(Error::Unsupported(_))
    ));
}

#[tokio::test]
async fn complete_envelope_is_bounded_even_for_one_huge_block() {
    let root = fixture_root();
    let mut backend = fixture();
    backend.text = "a".repeat(10_000);
    let tool = OcrTool::new(root.path(), OcrEngine::new(backend).unwrap())
        .await
        .unwrap()
        .with_limits(OcrLimits {
            max_output_bytes: 2048,
            ..Default::default()
        })
        .unwrap();
    let result = tool
        .ocr(input("image.png"), &CancellationToken::new())
        .await
        .unwrap();
    assert!(serde_json::to_vec(&result).unwrap().len() <= 2048);
    assert_eq!(result.data.omitted_blocks, 2);
    assert!(result.data.blocks.is_empty() && result.data.truncated);
}

#[tokio::test]
async fn cancelling_a_caller_keeps_the_shared_slot_until_native_work_ends() {
    let root = fixture_root();
    let mut backend = fixture();
    let calls = Arc::clone(&backend.calls);
    let entered = Arc::new(tokio::sync::Notify::new());
    let (release, receiver) = mpsc::channel();
    backend.entered = Some(Arc::clone(&entered));
    backend.release = Some(receiver);
    let engine = OcrEngine::new(backend).unwrap();
    let tool = OcrTool::new(root.path(), engine.clone()).await.unwrap();
    let other = OcrTool::new(root.path(), engine).await.unwrap();
    let cancel = CancellationToken::new();
    let job = tokio::spawn({
        let tool = tool.clone();
        let cancel = cancel.clone();
        async move { tool.ocr(input("image.png"), &cancel).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert!(matches!(job.await.unwrap(), Err(Error::Cancelled)));
    let mut request = input("image.png");
    request.timeout_ms = Some(20);
    assert!(matches!(
        other.ocr(request, &CancellationToken::new()).await,
        Err(Error::LimitExceeded(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    other
        .ocr(input("image.png"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn results_name_the_image_relative_to_the_workspace() {
    let root = fixture_root();
    std::fs::create_dir(root.path().join("scans")).unwrap();
    std::fs::copy(
        root.path().join("image.png"),
        root.path().join("scans/page.png"),
    )
    .unwrap();
    let tool = OcrTool::new(root.path(), OcrEngine::new(fixture()).unwrap())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let absolute = root.path().join("image.png");
    for (request, expected) in [
        ("image.png", "image.png"),
        ("scans/page.png", "scans/page.png"),
        (absolute.to_str().unwrap(), "image.png"),
    ] {
        let result = tool.ocr(input(request), &cancel).await.unwrap();
        assert_eq!(result.data.path, expected);
        assert!(result.text.starts_with(&format!("OCR {expected:?}")));
    }
}
