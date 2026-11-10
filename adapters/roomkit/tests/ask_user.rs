//! `ask_user` served by RoomKit's human input, through a real kit: the model
//! is offered the tool, the host's hook reads the questions and answers, or
//! refuses with what the model should fix. Python's counterpart is
//! `tests/python/test_roomkit_ask_user.py`.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture errors must fail tests"
)]

#[expect(dead_code, reason = "this test takes only the module's `ask`")]
mod call;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use roomkit::human_input::PendingInputEvent;
use roomkit::model::hook::AllowOptions;
use roomkit::model::{ChannelId, HookResult, RoomId};
use roomkit::{
    HookExecution, HookOptions, HookTrigger, RejectInputOptions, RoomKit, RoomKitOptions,
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{
    AiChannel, AiChannelOptions, AiContentPart, AiMessageContent, AiToolResultPart, ToolResult,
};
use roomkit_tools_agents::{AskUserQuestion, AskUserTool};
use roomkit_tools_roomkit::ask_user_handler;
use serde_json::{Value, json};

/// The questions the host's hook showed.
type Shown = Arc<Mutex<Vec<Vec<AskUserQuestion>>>>;

/// The host's view: read the questions, answer with the first option, or
/// refuse with what the model should fix.
fn show(kit: &RoomKit, shown: Shown) {
    let engine = kit.downgrade();
    kit.hook(
        HookTrigger::OnUserInputRequired,
        move |event: PendingInputEvent, _context| {
            let (engine, shown) = (engine.clone(), Arc::clone(&shown));
            async move {
                let kit = engine.upgrade().expect("the kit is open");
                match AskUserTool::read(&event.arguments) {
                    Ok(input) => {
                        let first = &input.questions[0];
                        let answer = format!("{}: {}", first.header, first.options[0].label);
                        shown
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(input.questions);
                        kit.answer_input(&event.pending_id, &answer).await.unwrap();
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        let options = RejectInputOptions { reason };
                        kit.reject_input(&event.pending_id, options).await.unwrap();
                    }
                }
                HookResult::allow(AllowOptions::default())
            }
        },
        HookOptions {
            execution: HookExecution::Sync,
            ..HookOptions::default()
        },
    )
    .unwrap();
}

/// A turn whose model calls `ask_user` with `arguments`: the model's
/// requests and the questions the host showed.
async fn turn(arguments: &Value) -> (ScriptedProvider, Vec<Vec<AskUserQuestion>>) {
    let arguments = arguments.to_string();
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("call-ask", "ask_user", arguments.as_str())]),
        ScriptedRound::answering("Noted."),
    ]);
    let options = AiChannelOptions {
        human_input_handler: Some(ask_user_handler(Duration::from_secs(5)).unwrap()),
        ..AiChannelOptions::default()
    };
    let agent = ChannelId::new("assistant").unwrap();
    let kit = RoomKit::new(RoomKitOptions::default());
    kit.register_channel(AiChannel::new(agent.clone(), model.clone(), options).unwrap())
        .unwrap();
    let shown = Shown::default();
    show(&kit, Arc::clone(&shown));
    call::ask(&kit, &RoomId::new("room").unwrap(), &agent).await;
    kit.close().await.unwrap();
    let shown = std::mem::take(&mut *shown.lock().unwrap_or_else(PoisonError::into_inner));
    (model, shown)
}

/// The text of a call's `result`, as the model reads it.
fn text(result: &ToolResult) -> String {
    match result {
        ToolResult::Text(text) => text.clone(),
        ToolResult::Json(value) => value.to_string(),
        ToolResult::Parts(parts) => format!("{parts:?}"),
    }
}

/// What the model read of its call, in its second request.
fn read_by_model(model: &ScriptedProvider) -> AiToolResultPart {
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "the model asks, then answers");
    requests[1]
        .messages
        .iter()
        .find_map(|message| match &message.content {
            AiMessageContent::Parts(parts) => parts.iter().find_map(|part| match part {
                AiContentPart::ToolResult(result) => Some(result.clone()),
                _ => None,
            }),
            AiMessageContent::Text(_) => None,
        })
        .expect("the request carries the call's result")
}

#[tokio::test]
async fn the_model_is_offered_the_tool_and_reads_the_answer() {
    let arguments = json!({"questions": [{
        "header": "Database",
        "question": "Which database?",
        "options": [{"label": "SQLite"}, {"label": "Postgres"}]
    }]});
    let (model, shown) = turn(&arguments).await;
    let offered = model.requests()[0]
        .tools
        .iter()
        .find(|tool| tool.name == "ask_user")
        .cloned()
        .expect("the model is offered ask_user");
    assert_eq!(offered.parameters, AskUserTool::definition().input_schema);
    assert_eq!(shown[0][0].question, "Which database?");
    let result = read_by_model(&model);
    assert_eq!(text(&result.result), "Database: SQLite");
    assert!(!result.is_error);
}

#[tokio::test]
async fn the_model_reads_what_to_fix_in_its_call() {
    let (model, shown) = turn(&json!({"questions": [{"question": "  "}]})).await;
    assert!(shown.is_empty());
    let result = read_by_model(&model);
    assert!(result.is_error);
    let read = text(&result.result);
    assert!(read.contains("Question 1 has no text."), "{read}");
}

#[tokio::test]
async fn the_model_reads_that_its_call_is_too_long() {
    let label = "é".repeat(129);
    let arguments = json!({"questions": [{"question": "Q?", "options": [{"label": label}]}]});
    let (model, shown) = turn(&arguments).await;
    assert!(shown.is_empty());
    let result = read_by_model(&model);
    assert!(result.is_error);
    let read = text(&result.result);
    assert!(
        read.contains("An option label of question 1 exceeds 256 bytes."),
        "{read}"
    );
}
