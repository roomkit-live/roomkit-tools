//! The optional OCR adapter exposes the shared schema and maps failures.
#![cfg(feature = "ocr")]
use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError};
use roomkit_tools_ocr::{DecodedImage, OcrBackend, OcrBlock, OcrEngine, OcrIdentity, OcrTool};
use roomkit_tools_roomkit::Ocr;
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct Fixture;
impl OcrBackend for Fixture {
    fn identity(&self) -> OcrIdentity {
        OcrIdentity {
            provider: "fixture".into(),
            model: "empty".into(),
            revision: None,
        }
    }
    fn recognize(
        &mut self,
        _: &DecodedImage,
        _: &CancellationToken,
    ) -> Result<Vec<OcrBlock>, roomkit_tools_core::ToolError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn optional_schema_validation_and_declined_calls() {
    let root = tempfile::tempdir().unwrap();
    let native = OcrTool::new(root.path(), OcrEngine::new(Fixture).unwrap())
        .await
        .unwrap();
    let tool = Ocr::new(native);
    assert_eq!(tool.definition().name, "ocr");
    assert_eq!(
        tool.definition().parameters,
        OcrTool::definition().input_schema
    );
    let arguments: Metadata =
        serde_json::from_value(json!({"path":"page.png", "rotation_degrees":45})).unwrap();
    assert!(matches!(
        tool.handler("ocr".into(), arguments.clone()).await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(
        tool.handler("read".into(), arguments).await,
        Err(ToolError::UnservedToolCall(_))
    ));
}
