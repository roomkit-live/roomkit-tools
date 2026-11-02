//! Every tool's per-call `timeout_ms` is a positive number of milliseconds:
//! its schema says so to the model, and 0 is refused when the input is read.
#![forbid(unsafe_code)]
#![expect(
    clippy::indexing_slicing,
    reason = "test assertions on JSON the tools themselves derive"
)]

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

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
    refuses_zero::<roomkit_tools_code::CodeOutlineInput>(json!({"path": "a.rs"}));
    refuses_zero::<roomkit_tools_ocr::OcrInput>(json!({"path": "a.png"}));
    refuses_zero::<roomkit_tools_image::ViewImageInput>(json!({"path": "a.png"}));
    refuses_zero::<roomkit_tools_search::WebSearchInput>(json!({"query": "q"}));
    refuses_zero::<roomkit_tools_web::WebFetchInput>(json!({"url": "https://example.com"}));
}

#[test]
fn each_schema_tells_the_model_the_timeout_is_positive() {
    for (definition, tool) in [
        (
            roomkit_tools_code::CodeOutlineTool::definition(),
            "code_outline",
        ),
        (roomkit_tools_ocr::OcrTool::definition(), "ocr"),
        (
            roomkit_tools_image::ViewImageTool::definition(),
            "view_image",
        ),
        (
            roomkit_tools_search::WebSearchTool::definition(),
            "web_search",
        ),
        (roomkit_tools_web::WebFetchTool::definition(), "web_fetch"),
    ] {
        says_positive(&definition.input_schema, tool);
    }
}
