//! The bridge every adapter shares: RoomKit's `Tool` definition, argument
//! decoding, error mapping, call cancellation and the trusted call owner.

use std::future::Future;

use roomkit::model::{Metadata, ToolCall};
use roomkit_ai::{AiTool, ToolError, ToolFuture, ToolResult, current_tool_call};
use roomkit_tools_core::{Definition, Error};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

/// The native definition as RoomKit declares it: its input schema, unchanged,
/// as the tool's parameters.
pub(crate) fn ai_tool(definition: Definition) -> AiTool {
    AiTool {
        name: definition.name,
        description: definition.description,
        parameters: definition.input_schema,
    }
}

/// Serve the call `name` with `arguments` for the tool named `wire`.
///
/// A call to another tool is not this adapter's to serve, so RoomKit hands it
/// to the next tool. Arguments that do not fit the input are refused, as
/// RoomKit refuses them. `run` gets a token cancelled when RoomKit drops the
/// call; its output reaches the model as JSON.
pub(crate) fn serve<I, O, F, Fut>(
    wire: &'static str,
    name: String,
    arguments: Metadata,
    run: F,
) -> ToolFuture
where
    I: DeserializeOwned,
    O: Serialize,
    F: FnOnce(I, CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
{
    Box::pin(async move {
        if name != wire {
            return Err(ToolError::unserved(format!(
                "'{wire}' does not serve '{name}'"
            )));
        }
        let id = current_tool_call()
            .map(|call| call.tool_call_id)
            .unwrap_or_default();
        let input = roomkit_ai::arguments(&ToolCall::new(id, name, arguments))?;
        let cancel = CancellationToken::new();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let output = run(input, cancel).await?;
        serde_json::to_value(output)
            .map(ToolResult::Json)
            .map_err(|error| ToolError::raised(&error))
    })
}

/// The room and the acting agent of the current RoomKit tool call.
///
/// Model arguments never select them. Outside a RoomKit tool call, the
/// `tools` family refuses, as Python's adapter does.
pub(crate) fn current_caller(tools: &str) -> Result<(String, String), ToolError> {
    current_tool_call()
        .and_then(|call| {
            let room = call.room_id?.to_string();
            let agent = call.channel_id?.to_string();
            Some((room, agent))
        })
        .ok_or_else(|| {
            ToolError::refused(format!(
                "{tools} tools require a conversation and an acting agent"
            ))
        })
}

/// The owner key of the current call: see [`roomkit_owner`](crate::roomkit_owner).
pub(crate) fn current_owner(tools: &str) -> Result<String, ToolError> {
    let (room, agent) = current_caller(tools)?;
    Ok(crate::roomkit_owner(&room, &agent))
}

/// A native error as the model reads it: invalid input and access refusals
/// are refusals, limits and cancellation are failures.
pub(crate) fn tool_error(error: &Error) -> ToolError {
    match error {
        Error::InvalidInput(_) | Error::Refused(_) => ToolError::refused(error.to_string()),
        Error::LimitExceeded(_) | Error::Unsupported(_) | Error::Cancelled => {
            ToolError::failed(error.to_string())
        }
        // Diagnostic causes reach RoomKit's observers, never the model.
        _ => ToolError::raised(error),
    }
}
