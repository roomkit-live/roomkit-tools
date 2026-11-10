//! `ask_user`, served by RoomKit's human input rather than as a [`Tool`]:
//! the kit opens a request per call and waits for the person's answers.
//!
//! [`Tool`]: roomkit_ai::Tool

use std::time::Duration;

use roomkit_ai::{AiError, HumanInputToolHandler, HumanInputToolHandlerOptions};
use roomkit_tools_agents::AskUserTool;

use crate::bridge::ai_tool;

/// `ask_user` served by RoomKit's human input, each call waiting `timeout` at
/// most for the person's answers: the AI channel's `human_input_handler`.
///
/// Each call opens a request and fires `OnUserInputRequired`. The host reads
/// its questions with [`AskUserTool::read`] from the event's arguments, shows
/// them, and settles the request with `RoomKit::answer_input`, or with
/// `RoomKit::reject_input` and the reason, which the model reads.
///
/// # Errors
/// The kit refusing the tool's definition, which a valid build never does.
pub fn ask_user_handler(timeout: Duration) -> Result<HumanInputToolHandler, AiError> {
    let declared = ai_tool(AskUserTool::definition());
    let name = declared.name.clone();
    let options = HumanInputToolHandlerOptions {
        timeout,
        tool_definitions: vec![declared],
        ..HumanInputToolHandlerOptions::default()
    };
    HumanInputToolHandler::new([name], options)
}
