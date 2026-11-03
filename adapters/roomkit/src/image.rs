//! The image adapter answers parts: metadata a model reads, pixels it sees.

use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{
    AiImagePart, AiTextPart, AiTool, AiToolResultPart, Tool, ToolError, ToolFuture, ToolResult,
    ToolResultPart,
};
use roomkit_tools_core::Output;
use roomkit_tools_image::{ViewImageResult, ViewImageTool};
use serde_json::{Value, json};

use crate::bridge::{ai_tool, anyone, serve_result, tool_error};

/// The `view_image` tool: local image pixels as RoomKit tool output.
///
/// Declare it for a model that sees: its handler answers a text part of the
/// metadata and an image part of the real pixels, never base64 in the text. A
/// model that does not see reads the text and `[image]`. Static PNG, JPEG and
/// WebP only; no OCR or remote URLs. Dropping a call cancels its token;
/// decoding that has started may finish in the blocking pool.
#[derive(Clone, Debug)]
pub struct ViewImage {
    tool: Arc<ViewImageTool>,
}

impl ViewImage {
    /// Adapt a configured tool without expanding its root or limits.
    pub fn new(tool: ViewImageTool) -> Self {
        Self {
            tool: Arc::new(tool),
        }
    }
}

impl Tool for ViewImage {
    fn definition(&self) -> AiTool {
        ai_tool(ViewImageTool::definition())
    }

    fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
        let tool = Arc::clone(&self.tool);
        serve_result(
            "view_image",
            name,
            arguments,
            anyone,
            move |(), input, cancel| async move {
                let output = tool.view(input, &cancel).await;
                parts(output.map_err(|error| tool_error(&error))?)
            },
        )
    }
}

/// The text part and the image part of `output`, as Python's handler answers
/// them: the data without its image and the text, as the JSON Python dumps,
/// then the pixels.
fn parts(output: Output<ViewImageResult>) -> Result<ToolResult, ToolError> {
    let Output { data, text, .. } = output;
    let image = AiImagePart {
        url: data.image.data_url(),
        mime_type: Some(data.image.mime_type.as_str().to_owned()),
    };
    let mut metadata = serde_json::to_value(data).map_err(|error| ToolError::raised(&error))?;
    if let Some(fields) = metadata.as_object_mut() {
        // Keeps the other fields' order, as Python's `pop`; `remove` swaps.
        fields.shift_remove("image");
    }
    let body = dumps(json!({ "data": metadata, "text": text }));
    Ok(ToolResult::Parts(vec![
        ToolResultPart::Text(AiTextPart::new(body)),
        ToolResultPart::Image(image),
    ]))
}

/// The object `value` as Python's `json.dumps(value, ensure_ascii=False)`
/// writes it: the text RoomKit gives every other adapter's JSON result.
fn dumps(value: Value) -> String {
    AiToolResultPart::new(String::new(), String::new(), value).as_text()
}
