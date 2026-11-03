//! One call of a tool by a mock model, through a real kit, as an integrator
//! declares the tool. A test using it also declares `mod call;`.

use std::sync::Arc;

use roomkit::RoomKit;
use roomkit::model::{ChannelId, Metadata, RoomId};
use roomkit_ai::{
    AiChannel, AiChannelOptions, AiContentPart, AiContext, AiMessageContent, AiResponse,
    AiToolCall, AiToolResultPart, MockAiProvider, MockAiProviderOptions, Tool, ToolEntry,
};

/// A model of `vision` that calls the tool `name` with `arguments`, then answers.
fn model_calling(vision: bool, name: &str, arguments: Metadata) -> MockAiProvider {
    let call = AiToolCall {
        arguments,
        ..AiToolCall::new("call-1", name)
    };
    MockAiProvider::new(MockAiProviderOptions {
        vision,
        ai_responses: Some(vec![
            AiResponse {
                finish_reason: Some("tool_calls".into()),
                tool_calls: vec![call],
                ..AiResponse::new("")
            },
            AiResponse::new("Done"),
        ]),
        ..MockAiProviderOptions::default()
    })
}

/// The tool's result in the request `context`.
fn tool_result_in(context: &AiContext) -> AiToolResultPart {
    context
        .messages
        .iter()
        .find_map(|message| match &message.content {
            AiMessageContent::Parts(parts) => parts.iter().find_map(|part| match part {
                AiContentPart::ToolResult(result) => Some(result.clone()),
                _ => None,
            }),
            AiMessageContent::Text(_) => None,
        })
        .expect("the request carries the tool's result")
}

/// What the model of `vision` reads of its call of `tool` as `name`, in `kit`,
/// which it closes.
pub async fn model_reads(
    kit: &RoomKit,
    tool: Arc<dyn Tool>,
    vision: bool,
    name: &str,
    arguments: Metadata,
) -> AiToolResultPart {
    let model = model_calling(vision, name, arguments);
    let agent = ChannelId::new("assistant").unwrap();
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Tool(tool)],
        ..AiChannelOptions::default()
    };
    kit.register_channel(AiChannel::new(agent.clone(), model.clone(), options).unwrap())
        .unwrap();
    crate::call::ask(kit, &RoomId::new("room").unwrap(), &agent).await;
    kit.close().await.unwrap();
    let requests = model.calls();
    assert_eq!(requests.len(), 2, "the model calls the tool, then answers");
    tool_result_in(&requests[1])
}
