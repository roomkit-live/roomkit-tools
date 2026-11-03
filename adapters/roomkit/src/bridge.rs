//! The bridge every adapter shares: RoomKit's `Tool` definition, argument
//! decoding, error mapping, call cancellation and the trusted call owner.

use std::future::Future;

use roomkit::model::{Metadata, ToolCall};
use roomkit_ai::{AiTool, ToolError, ToolFuture, ToolResult, current_tool_call};
use roomkit_tools_core::Definition;
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

/// Serve the call `name` with `arguments` for the tool named `wire`, as
/// [`serve_result`] does; `run`'s output reaches the model as JSON.
pub(crate) fn serve<C, I, O, A, F, Fut>(
    wire: &'static str,
    name: String,
    arguments: Metadata,
    caller: A,
    run: F,
) -> ToolFuture
where
    I: DeserializeOwned,
    O: Serialize,
    A: FnOnce() -> Result<C, ToolError> + Send + 'static,
    C: Send + 'static,
    F: FnOnce(C, I, CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
{
    let run = move |caller, input, cancel| {
        let running = run(caller, input, cancel);
        async move {
            serde_json::to_value(running.await?)
                .map(ToolResult::Json)
                .map_err(|error| ToolError::raised(&error))
        }
    };
    serve_result(wire, name, arguments, caller, run)
}

/// Serve the call `name` with `arguments` for the tool named `wire`.
///
/// A call to another tool is not this adapter's to serve, so RoomKit hands it
/// to the next tool. `caller` then names who calls, from RoomKit's current
/// call, or refuses, before the arguments are read, as Python's adapter
/// checks the owner first. Arguments that do not fit the input are refused,
/// as RoomKit refuses them. `run` gets the caller and a token cancelled when
/// RoomKit drops the call, and answers what the model reads.
pub(crate) fn serve_result<C, I, A, F, Fut>(
    wire: &'static str,
    name: String,
    arguments: Metadata,
    caller: A,
    run: F,
) -> ToolFuture
where
    I: DeserializeOwned,
    A: FnOnce() -> Result<C, ToolError> + Send + 'static,
    C: Send + 'static,
    F: FnOnce(C, I, CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<ToolResult, ToolError>> + Send + 'static,
{
    Box::pin(async move {
        if name != wire {
            return Err(ToolError::unserved(format!(
                "'{wire}' does not serve '{name}'"
            )));
        }
        let caller = caller()?;
        // The call's id plays no part in reading its arguments.
        let input = roomkit_ai::arguments(&ToolCall::new(String::new(), name, arguments))?;
        let cancel = CancellationToken::new();
        let _cancel_on_drop = cancel.clone().drop_guard();
        run(caller, input, cancel).await
    })
}

/// The caller of an adapter any call may use: nobody in particular.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a caller step answers as every other one does"
)]
pub(crate) fn anyone() -> Result<(), ToolError> {
    Ok(())
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

/// A native error as Python's binding and adapters give it to RoomKit, by its
/// variant, in its public words, which leave out OS details and host paths:
/// invalid input and access refusals are refusals, limits and cancellation are
/// failures. The rest raises Python's class: the model reads the class, the
/// observers the class and the words.
pub(crate) fn tool_error(error: &roomkit_tools_core::ToolError) -> ToolError {
    let words = error.to_string();
    match error {
        roomkit_tools_core::ToolError::InvalidInput(_)
        | roomkit_tools_core::ToolError::AccessRefused(_) => ToolError::refused(words),
        roomkit_tools_core::ToolError::LimitExceeded(_)
        | roomkit_tools_core::ToolError::UnsupportedResource(_)
        | roomkit_tools_core::ToolError::Cancelled => ToolError::failed(words),
        roomkit_tools_core::ToolError::Io { .. } => ToolError::raised_as("ToolIOError", words),
        _ => ToolError::raised_as("ToolError", words),
    }
}
