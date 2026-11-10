//! The optional `view_image` adapter answers parts, as Python's `ViewImage`.
//!
//! The expected bytes are in the fixture Python's adapter is checked against
//! too: `tests/python/test_roomkit_image.py`.
#![cfg(all(unix, feature = "image"))]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture errors must fail tests"
)]

#[expect(dead_code, reason = "this test takes only the module's `ask`")]
mod call;
mod scripted;

use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use roomkit::model::Metadata;
use roomkit::{RoomKit, RoomKitOptions};
use roomkit_ai::{
    AiImagePart, AiTextPart, AiToolResultPart, Tool, ToolError, ToolFuture, ToolResult,
    ToolResultPart,
};
use roomkit_tools_image::{ViewImageLimits, ViewImageTool};
use roomkit_tools_roomkit::ViewImage;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/roomkit_view_image.json"
));

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

/// A workspace holding the fixture's real PNG, at the fixture's path.
fn workspace() -> tempfile::TempDir {
    let fixture = fixture();
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join(fixture["path"].as_str().unwrap());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let png = STANDARD.decode(fixture["png"].as_str().unwrap()).unwrap();
    std::fs::write(target, png).unwrap();
    root
}

fn arguments(path: &str) -> Metadata {
    serde_json::from_value(json!({ "path": path })).unwrap()
}

/// The parts both adapters answer for the fixture's file.
fn expected_parts() -> Vec<ToolResultPart> {
    let fixture = fixture();
    vec![
        ToolResultPart::Text(AiTextPart::new(fixture["text_part"].as_str().unwrap())),
        ToolResultPart::Image(AiImagePart {
            url: fixture["image_part"]["url"].as_str().unwrap().to_owned(),
            mime_type: Some(
                fixture["image_part"]["mime_type"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
        }),
    ]
}

#[tokio::test]
async fn definition_is_the_native_schema_and_another_name_is_unserved() {
    let root = workspace();
    let tool = ViewImage::new(ViewImageTool::new(root.path()).await.unwrap());
    let definition = tool.definition();
    let native = ViewImageTool::definition();
    assert_eq!(definition.name, native.name);
    assert_eq!(definition.description, native.description);
    assert_eq!(definition.parameters, native.input_schema);
    assert_eq!(
        tool.handler("read".into(), arguments("colors.png")).await,
        Err(ToolError::UnservedToolCall(
            "'view_image' does not serve 'read'".into()
        ))
    );
}

#[tokio::test]
async fn answers_the_text_part_and_image_part_python_answers() {
    let root = workspace();
    let tool = ViewImage::new(ViewImageTool::new(root.path()).await.unwrap());
    let path = fixture()["path"].as_str().unwrap().to_owned();
    let answer = tool
        .handler("view_image".into(), arguments(&path))
        .await
        .unwrap();
    assert_eq!(answer, ToolResult::Parts(expected_parts()));
}

/// `call`, polled once: it has taken a worker slot and waits on its file.
/// Nothing polls it further, so it keeps the slot until it is dropped.
async fn started(mut call: ToolFuture) -> ToolFuture {
    let answered = tokio::select! {
        biased;
        _ = &mut call => true,
        () = std::future::ready(()) => false,
    };
    assert!(!answered, "the call waits on its file");
    call
}

/// RoomKit cancels a call by dropping it. A call runs only as its caller polls
/// it, and dropped, it gives up its slot. Where a dropped call in its decoder
/// stops is `ViewImageTool::view`'s own guard: it has no effect a test can read.
#[tokio::test]
async fn a_dropped_call_gives_up_its_worker_slot() {
    let root = workspace();
    let path = fixture()["path"].as_str().unwrap().to_owned();
    let tool = ViewImage::new(ViewImageTool::new(root.path()).await.unwrap());
    let holding = [
        started(tool.handler("view_image".into(), arguments(&path))).await,
        started(tool.handler("view_image".into(), arguments(&path))).await,
    ];
    let queued: Metadata =
        serde_json::from_value(json!({ "path": path, "timeout_ms": 50 })).unwrap();
    assert_eq!(
        tool.handler("view_image".into(), queued).await,
        Err(ToolError::Failed(
            "Limit exceeded: view_image deadline exceeded".into()
        )),
        "both slots stay held while the calls are alive"
    );
    drop(holding);
    assert_eq!(
        tool.handler("view_image".into(), arguments(&path)).await,
        Ok(ToolResult::Parts(expected_parts()))
    );
}

#[tokio::test]
async fn refusals_and_io_errors_map_as_in_the_other_adapters() {
    let root = workspace();
    let fixture = fixture();
    let tool = ViewImage::new(ViewImageTool::new(root.path()).await.unwrap());
    for refusal in fixture["refusals"].as_array().unwrap() {
        let path = refusal["path"].as_str().unwrap();
        assert_eq!(
            tool.handler("view_image".into(), arguments(path)).await,
            Err(ToolError::Refused(
                refusal["message"].as_str().unwrap().to_owned()
            )),
            "{refusal}"
        );
    }
    // A missing file is refused, naming it.
    assert_eq!(
        tool.handler("view_image".into(), arguments("missing.png"))
            .await,
        Err(ToolError::Refused(
            "Invalid input: no such file or directory: missing.png".into()
        ))
    );
    // A file the tool cannot reach is the tool's error, in Python's class and
    // words: RoomKit gives the model the class and its observers the words.
    let below_a_file = format!("{}/child.png", fixture["path"].as_str().unwrap());
    assert_eq!(
        tool.handler("view_image".into(), arguments(&below_a_file))
            .await,
        Err(ToolError::Raised {
            kind: "ToolIOError".into(),
            message: "Unable to resolve the image path".into(),
        })
    );
}

#[tokio::test]
async fn a_limit_is_a_failure_in_python_s_words() {
    let root = workspace();
    let fixture = fixture();
    let small = ViewImageLimits {
        max_image_bytes: usize::try_from(fixture["failure"]["max_image_bytes"].as_u64().unwrap())
            .unwrap(),
        ..ViewImageLimits::default()
    };
    let limited = ViewImageTool::new(root.path())
        .await
        .unwrap()
        .with_limits(small)
        .unwrap();
    let path = fixture["path"].as_str().unwrap();
    assert_eq!(
        ViewImage::new(limited)
            .handler("view_image".into(), arguments(path))
            .await,
        Err(ToolError::Failed(
            fixture["failure"]["message"].as_str().unwrap().to_owned()
        ))
    );
}

/// What the model of `vision` reads of its call of `view_image` on `path`.
async fn model_reads(root: &Path, vision: bool, path: &str) -> AiToolResultPart {
    let tool = ViewImage::new(ViewImageTool::new(root).await.unwrap());
    let kit = RoomKit::new(RoomKitOptions::default());
    scripted::model_reads(&kit, Arc::new(tool), vision, "view_image", arguments(path)).await
}

#[tokio::test]
async fn a_model_that_sees_receives_the_image_part_in_its_request() {
    let root = workspace();
    let path = fixture()["path"].as_str().unwrap().to_owned();
    let read = model_reads(root.path(), true, &path).await;
    assert!(!read.is_error);
    let ToolResult::Parts(parts) = &read.result else {
        panic!("a model that sees reads parts, got {:?}", read.result);
    };
    assert_eq!(parts, &expected_parts());
    let ToolResultPart::Image(image) = &parts[1] else {
        panic!("the second part is the image");
    };
    let encoded = image.url.strip_prefix("data:image/png;base64,").unwrap();
    assert!(
        STANDARD
            .decode(encoded)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
}

#[tokio::test]
async fn a_model_without_vision_reads_the_text_and_an_image_mark() {
    let root = workspace();
    let fixture = fixture();
    let read = model_reads(root.path(), false, fixture["path"].as_str().unwrap()).await;
    assert!(!read.is_error);
    let text = fixture["text_part"].as_str().unwrap();
    assert_eq!(read.result, ToolResult::Text(format!("{text}\n[image]")));
}

#[tokio::test]
async fn a_refused_path_reaches_the_model_in_python_s_words() {
    let root = workspace();
    let fixture = fixture();
    for refusal in fixture["refusals"].as_array().unwrap() {
        let path = refusal["path"].as_str().unwrap();
        let read = model_reads(root.path(), true, path).await;
        assert!(read.is_error, "{refusal}");
        assert_eq!(
            read.result,
            ToolResult::Text(refusal["message"].as_str().unwrap().to_owned()),
            "{refusal}"
        );
    }
}
