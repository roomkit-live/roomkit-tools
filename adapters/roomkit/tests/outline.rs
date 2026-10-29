//! The independently selected outline adapter preserves its native contract.
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_code::CodeOutlineTool;
use roomkit_tools_roomkit::CodeOutlineToolset;
use serde_json::json;

#[tokio::test]
async fn definitions_results_refusal_and_close_follow_the_native_outline() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source.rs"),
        "struct Service;\nimpl Service { fn run(&self) {} }\n",
    )
    .unwrap();
    let tool = CodeOutlineToolset::new(CodeOutlineTool::new(root.path()).await.unwrap());
    assert_eq!(tool.tools().len(), 1);
    assert_eq!(
        tool.tools()[0].parameters,
        CodeOutlineTool::definition().input_schema
    );
    let context = ToolContext::new(
        RoomId::new("outline").unwrap(),
        ChannelId::new("agent").unwrap(),
    );
    let call = ToolCall::new(
        "outline",
        "code_outline",
        serde_json::from_value(json!({"path":"source.rs"})).unwrap(),
    );
    let ToolOutput::Json(result) = tool.call(call.clone(), context.clone()).await.unwrap() else {
        panic!("expected structured result");
    };
    assert_eq!(result["data"]["symbols"][2]["name"], "run");
    assert_eq!(result["data"]["symbols"][2]["parent"], 2);
    assert_eq!(result["data"]["syntax_errors"], false);
    let other = ToolCall::new("other", "read", call.arguments.clone());
    assert!(matches!(
        tool.call(other, context.clone()).await,
        Err(ToolError::Refused(_))
    ));
    tool.close().await;
    assert!(matches!(
        tool.call(call, context).await,
        Err(ToolError::Refused(_))
    ));
}
