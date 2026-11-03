//! Each native error an adapter maps reaches the handler's caller, the model
//! and the host in the words Python's adapters give them.
//!
//! The expected words are in the fixture Python's adapters are checked against
//! too: `tests/python/test_roomkit_errors.py`.
#![cfg(all(unix, feature = "image"))]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "fixture errors must fail tests"
)]

#[expect(dead_code, reason = "this test takes only the module's `ask`")]
mod call;
mod scripted;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use roomkit::model::HookResult;
use roomkit::model::Metadata;
use roomkit::model::hook::AllowOptions;
use roomkit::{HookExecution, HookOptions, HookTrigger, RoomKit, RoomKitOptions, ToolCallEvent};
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_fs::{ReadLimits, ReadTool};
use roomkit_tools_image::{ViewImageLimits, ViewImageTool};
use roomkit_tools_roomkit::{Read, ViewImage};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/roomkit_tool_errors.json"
));

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn workspace(fixture: &Value) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, data) in fixture["files"].as_object().unwrap() {
        let bytes = STANDARD.decode(data.as_str().unwrap()).unwrap();
        std::fs::write(root.path().join(name), bytes).unwrap();
    }
    root
}

fn arguments(case: &Value) -> Metadata {
    serde_json::from_value(case["arguments"].clone()).unwrap()
}

/// The adapter a case calls, with its limits.
async fn adapter(root: &Path, case: &Value) -> Arc<dyn Tool> {
    let limit = |name: &str, otherwise: usize| {
        case["limits"][name]
            .as_u64()
            .map_or(otherwise, |value| usize::try_from(value).unwrap())
    };
    match case["tool"].as_str().unwrap() {
        "view_image" => {
            let limits = ViewImageLimits::default();
            let limits = ViewImageLimits {
                max_image_bytes: limit("max_image_bytes", limits.max_image_bytes),
                ..limits
            };
            let tool = ViewImageTool::new(root).await.unwrap();
            Arc::new(ViewImage::new(tool.with_limits(limits).unwrap()))
        }
        "read" => {
            let limits = ReadLimits::default();
            let limits = ReadLimits {
                max_file_bytes: limit("max_file_bytes", limits.max_file_bytes),
                ..limits
            };
            let tool = ReadTool::new(root).await.unwrap();
            Arc::new(Read::new(tool.with_limits(limits).unwrap()))
        }
        other => panic!("no adapter for {other}"),
    }
}

/// The name a case's fixture gives an error: Python's class, which is
/// RoomKit's for a refusal and a failure.
fn raised(error: ToolError) -> Value {
    let (kind, message) = match error {
        ToolError::Refused(message) => ("ToolRefusedError".to_owned(), message),
        ToolError::Failed(message) => ("ToolFailedError".to_owned(), message),
        ToolError::Raised { kind, message } => (kind, message),
        other => panic!("not an error a native tool gives: {other:?}"),
    };
    json!({ "raises": kind, "message": message })
}

#[tokio::test]
async fn each_native_error_gives_the_handler_python_s_class_and_words() {
    let fixture = fixture();
    let root = workspace(&fixture);
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["tool"].as_str().unwrap().to_owned();
        let tool = adapter(root.path(), case).await;
        let error = tool.handler(name, arguments(case)).await.unwrap_err();
        let expected = json!({ "raises": case["raises"], "message": case["message"] });
        let actual = raised(error);
        if actual != expected {
            differences.push((
                case["variant"].clone(),
                case["tool"].clone(),
                actual,
                expected,
            ));
        }
    }
    assert!(differences.is_empty(), "{differences:#?}");
}

/// What the model and the host's `ON_TOOL_CALL` hook read of a case's call,
/// through a real kit, as its fixture writes it.
async fn read_through_a_kit(root: &Path, case: &Value) -> Value {
    let name = case["tool"].as_str().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let kit = RoomKit::new(RoomKitOptions::default());
    let keeping = Arc::clone(&seen);
    kit.hook(
        HookTrigger::OnToolCall,
        move |event: ToolCallEvent, _context| {
            *keeping.lock().unwrap_or_else(PoisonError::into_inner) = Some(event);
            async { HookResult::allow(AllowOptions::default()) }
        },
        HookOptions {
            name: "observe".into(),
            // A call that did not serve is reported to the observers.
            execution: HookExecution::Async,
            ..Default::default()
        },
    )
    .unwrap();
    let tool = adapter(root, case).await;
    let read = scripted::model_reads(&kit, tool, true, name, arguments(case)).await;
    let event = seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("the hook observes the call");
    let ToolResult::Text(model_reads) = read.result else {
        panic!("the model reads a failed call as text: {:?}", read.result);
    };
    assert!(read.is_error && event.is_error, "{case}");
    assert_eq!(event.result, Some(Value::String(model_reads.clone())));
    json!({
        "model_reads": model_reads,
        "error_detail": event.error_detail,
        "refused": event.refused,
    })
}

#[tokio::test]
async fn each_native_error_reaches_the_model_and_the_host_in_python_s_words() {
    let fixture = fixture();
    let root = workspace(&fixture);
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let expected = json!({
            "model_reads": case["model_reads"],
            "error_detail": case["error_detail"],
            "refused": case["refused"],
        });
        let actual = read_through_a_kit(root.path(), case).await;
        if actual != expected {
            differences.push((
                case["variant"].clone(),
                case["tool"].clone(),
                actual,
                expected,
            ));
        }
    }
    assert!(differences.is_empty(), "{differences:#?}");
}
