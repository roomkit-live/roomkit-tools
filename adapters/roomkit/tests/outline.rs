//! The independently selected outline adapter preserves its native contract.
mod served;

use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_code::CodeOutlineTool;
use roomkit_tools_roomkit::CodeOutline;
use serde_json::json;

#[tokio::test]
async fn definitions_results_and_declined_calls_follow_the_native_outline() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source.rs"),
        "struct Service;\nimpl Service { fn run(&self) {} }\n",
    )
    .unwrap();
    let tool = CodeOutline::new(CodeOutlineTool::new(root.path()).await.unwrap());
    assert_eq!(tool.definition().name, "code_outline");
    assert_eq!(
        tool.definition().parameters,
        CodeOutlineTool::definition().input_schema
    );
    let arguments: Metadata = serde_json::from_value(json!({"path":"source.rs"})).unwrap();
    let (text, result) = served::call(&tool, "code_outline", arguments.clone())
        .await
        .unwrap();
    assert!(matches!(text, ToolResult::Text(_)), "{text:?}");
    assert_eq!(result["symbols"][2]["name"], "run");
    assert_eq!(result["symbols"][2]["parent"], 2);
    assert_eq!(result["syntax_errors"], false);
    assert!(matches!(
        tool.handler("read".into(), arguments).await,
        Err(ToolError::UnservedToolCall(_))
    ));
}
