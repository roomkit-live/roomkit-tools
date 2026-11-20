//! A handler called within a RoomKit tool call, as the kit calls it, without
//! a kit: what it answers the model and the data it hands the host.

use roomkit::model::Metadata;
use roomkit_ai::{
    Tool, ToolCallContext, ToolError, ToolResult, ToolTurnContextOptions, tool_turn_context,
};
use serde_json::Value;

/// What `tool` answers a call of `name` with `arguments`: on success, the
/// result the model reads and the `data` of the call's structured content,
/// `{"data": ...}`, which the host reads.
#[expect(
    clippy::expect_used,
    reason = "a call outside the adapters' contract must fail the test"
)]
pub async fn call(
    tool: &dyn Tool,
    name: &str,
    arguments: Metadata,
) -> Result<(ToolResult, Value), ToolError> {
    let call = ToolCallContext::default();
    let options = ToolTurnContextOptions {
        call: Some(call.clone()),
        ..ToolTurnContextOptions::default()
    };
    let answered = tool_turn_context(options, tool.handler(name.to_owned(), arguments))
        .await
        .expect("the call's scope names no room");
    let result = answered?;
    let mut copy = call
        .structured_content
        .get()
        .expect("a served call leaves its data on its call")
        .to_metadata();
    assert_eq!(copy.len(), 1, "the copy holds the data alone: {copy:?}");
    let data = copy.remove("data").expect("the copy holds the data");
    Ok((result, data))
}
