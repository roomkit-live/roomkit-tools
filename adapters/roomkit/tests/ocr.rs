//! The optional OCR adapter exposes the shared schema and maps failures.
#![cfg(feature = "ocr")]
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, Toolset};
use roomkit_tools_core::Error;
use roomkit_tools_ocr::{DecodedImage, OcrBackend, OcrBlock, OcrEngine, OcrIdentity, OcrTool};
use roomkit_tools_roomkit::OcrToolset;
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
    ) -> Result<Vec<OcrBlock>, Error> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn optional_schema_validation_and_close() {
    let root = tempfile::tempdir().unwrap();
    let native = OcrTool::new(root.path(), OcrEngine::new(Fixture).unwrap())
        .await
        .unwrap();
    let tool = OcrToolset::new(native);
    assert_eq!(tool.tools().len(), 1);
    assert_eq!(
        tool.tools()[0].parameters,
        OcrTool::definition().input_schema
    );
    let context = ToolContext::new(
        RoomId::new("ocr").unwrap(),
        ChannelId::new("agent").unwrap(),
    );
    let call = ToolCall::new(
        "ocr",
        "ocr",
        serde_json::from_value(json!({"path":"page.png", "rotation_degrees":45})).unwrap(),
    );
    assert!(matches!(
        tool.call(call.clone(), context.clone()).await,
        Err(ToolError::Refused(_))
    ));
    tool.close().await;
    assert!(matches!(
        tool.call(call, context).await,
        Err(ToolError::Refused(_))
    ));
}
