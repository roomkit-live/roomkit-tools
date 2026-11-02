//! Every tool's per-call `timeout_ms` is a positive number of milliseconds:
//! its schema says so to the model, and 0 is refused when the input is read.
//! A value above the host maximum is refused with a message that gives it.
#![forbid(unsafe_code)]
#![expect(
    clippy::indexing_slicing,
    reason = "test assertions on JSON the tools themselves derive"
)]

use std::num::NonZeroU64;
use std::sync::Arc;

use roomkit_tools_code::{CodeOutlineInput, CodeOutlineLimits, CodeOutlineTool};
use roomkit_tools_core::ToolError;
use roomkit_tools_image::{ViewImageInput, ViewImageLimits, ViewImageTool};
use roomkit_tools_process::{
    ProcessLimits, ProcessSession, ProcessStartInput, ShellInput, ShellLimits, ShellTool,
};
use roomkit_tools_search::{
    ExaSearch, SearchHttpConfig, SearchLimits, WebSearchInput, WebSearchTool,
};
use roomkit_tools_web::{WebFetchInput, WebFetchLimits, WebFetchPolicy, WebFetchTool};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn refuses_zero<T: DeserializeOwned>(mut input: Value) {
    input["timeout_ms"] = json!(0);
    assert!(
        serde_json::from_value::<T>(input.clone()).is_err(),
        "{input}"
    );
    input["timeout_ms"] = json!(1);
    assert!(
        serde_json::from_value::<T>(input.clone()).is_ok(),
        "{input}"
    );
}

fn says_positive(schema: &Value, tool: &str) {
    let mut found = schema["properties"]["timeout_ms"].clone();
    if found.get("anyOf").is_some() {
        found = found["anyOf"][0].clone();
    }
    assert_eq!(found["minimum"], json!(1), "{tool}: {schema}");
}

#[test]
fn a_timeout_of_zero_is_refused_and_one_accepted() {
    refuses_zero::<CodeOutlineInput>(json!({"path": "a.rs"}));
    refuses_zero::<roomkit_tools_ocr::OcrInput>(json!({"path": "a.png"}));
    refuses_zero::<ViewImageInput>(json!({"path": "a.png"}));
    refuses_zero::<WebSearchInput>(json!({"query": "q"}));
    refuses_zero::<WebFetchInput>(json!({"url": "https://example.com"}));
}

#[test]
fn each_schema_tells_the_model_the_timeout_is_positive() {
    for (definition, tool) in [
        (CodeOutlineTool::definition(), "code_outline"),
        (roomkit_tools_ocr::OcrTool::definition(), "ocr"),
        (ViewImageTool::definition(), "view_image"),
        (WebSearchTool::definition(), "web_search"),
        (WebFetchTool::definition(), "web_fetch"),
    ] {
        says_positive(&definition.input_schema, tool);
    }
}

/// The host maximum every tool below is configured with, in milliseconds.
const MAX: u64 = 1_000;

/// Above the maximum, the refusal gives it, in one message for every tool;
/// at the maximum, the call passes the timeout check, and any refusal is for
/// something else.
fn refuses_above_the_maximum<T: std::fmt::Debug>(
    above: &Result<T, ToolError>,
    at: &Result<T, ToolError>,
) {
    let expected = format!("timeout_ms exceeds the host maximum of {MAX} ms");
    assert!(
        matches!(above, Err(ToolError::InvalidInput(message)) if *message == expected),
        "{above:?}"
    );
    if let Err(ToolError::InvalidInput(message)) = at {
        assert!(!message.contains("timeout_ms"), "{message}");
    }
}

fn ms(value: u64) -> Option<NonZeroU64> {
    NonZeroU64::new(value)
}

#[tokio::test]
async fn code_outline_refuses_a_timeout_above_the_maximum() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.py"), "def f():\n    pass\n").unwrap();
    let limits = CodeOutlineLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..CodeOutlineLimits::default()
    };
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let tool = tool.with_limits(limits).unwrap();
    let input = |timeout| CodeOutlineInput {
        timeout_ms: ms(timeout),
        ..CodeOutlineInput::new("a.py")
    };
    let cancel = CancellationToken::new();
    refuses_above_the_maximum(
        &tool.outline(input(MAX + 1), &cancel).await,
        &tool.outline(input(MAX), &cancel).await,
    );
}

#[tokio::test]
async fn view_image_refuses_a_timeout_above_the_maximum() {
    let root = tempfile::tempdir().unwrap();
    let limits = ViewImageLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..ViewImageLimits::default()
    };
    let tool = ViewImageTool::new(root.path()).await.unwrap();
    let tool = tool.with_limits(limits).unwrap();
    let input = |timeout| ViewImageInput {
        timeout_ms: ms(timeout),
        ..ViewImageInput::new("missing.png")
    };
    let cancel = CancellationToken::new();
    refuses_above_the_maximum(
        &tool.view(input(MAX + 1), &cancel).await,
        &tool.view(input(MAX), &cancel).await,
    );
}

#[tokio::test]
async fn web_fetch_refuses_a_timeout_above_the_maximum() {
    let limits = WebFetchLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..WebFetchLimits::default()
    };
    let policy = WebFetchPolicy {
        allowed_origins: Some(vec![]),
        ..WebFetchPolicy::default()
    };
    let tool = WebFetchTool::new().with_limits(limits).unwrap();
    let tool = tool.with_policy(policy).unwrap();
    let input = |timeout| WebFetchInput {
        timeout_ms: ms(timeout),
        ..WebFetchInput::new("https://example.com/")
    };
    let cancel = CancellationToken::new();
    refuses_above_the_maximum(
        &tool.fetch(input(MAX + 1), &cancel).await,
        &tool.fetch(input(MAX), &cancel).await,
    );
}

#[tokio::test]
async fn web_search_refuses_a_timeout_above_the_maximum() {
    let config = SearchHttpConfig {
        allowed_origins: Some(vec![]),
        ..SearchHttpConfig::default()
    };
    let limits = SearchLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..SearchLimits::default()
    };
    let tool = WebSearchTool::new(Arc::new(ExaSearch::new(config).unwrap()));
    let tool = tool.with_limits(limits).unwrap();
    let input = |timeout| WebSearchInput {
        timeout_ms: ms(timeout),
        ..WebSearchInput::new("q")
    };
    let cancel = CancellationToken::new();
    refuses_above_the_maximum(
        &tool.search(input(MAX + 1), &cancel).await,
        &tool.search(input(MAX), &cancel).await,
    );
}

#[tokio::test]
async fn shell_refuses_a_timeout_above_the_maximum() {
    let root = tempfile::tempdir().unwrap();
    let limits = ShellLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..ShellLimits::default()
    };
    let tool = ShellTool::new(root.path()).await.unwrap();
    let tool = tool.with_limits(limits).unwrap();
    let input = |timeout| ShellInput {
        timeout_ms: ms(timeout),
        ..ShellInput::new("true")
    };
    let cancel = CancellationToken::new();
    let above = tool.shell(input(MAX + 1), &cancel).await;
    let at = tool.shell(input(MAX), &cancel).await;
    assert_eq!(at.as_ref().unwrap().data.timeout_ms, MAX);
    refuses_above_the_maximum(&above, &at);
}

#[tokio::test]
async fn process_start_refuses_a_timeout_above_the_maximum() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        default_timeout_ms: 1_000,
        max_timeout_ms: 1_000,
        ..ProcessLimits::default()
    };
    let session = ProcessSession::new(root.path()).await.unwrap();
    let session = session.with_limits(limits).unwrap();
    let input = |timeout| ProcessStartInput {
        timeout_ms: ms(timeout),
        ..ProcessStartInput::new("true")
    };
    let cancel = CancellationToken::new();
    let above = session.start(None, input(MAX + 1), &cancel).await;
    let at = session.start(None, input(MAX), &cancel).await;
    assert_eq!(at.as_ref().unwrap().data.timeout_ms, MAX);
    refuses_above_the_maximum(&above, &at);
    session.close().await;
}
