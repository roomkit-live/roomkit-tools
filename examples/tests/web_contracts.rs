//! Compare complete Rust/Python fetch results against the same HTTP responses.
#![forbid(unsafe_code)]

#[path = "../../tests/support/web.rs"]
mod web;
use roomkit_tools_core::Error;
use roomkit_tools_web::{WebFetchPolicy, WebFetchTool};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn replace(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) => *s = s.replace(from, to),
        Value::Array(items) => items.iter_mut().for_each(|v| replace(v, from, to)),
        Value::Object(items) => items.values_mut().for_each(|v| replace(v, from, to)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[tokio::test]
async fn shared_web_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/web.json")).unwrap();
    let server = web::Server::start().await;
    let mut reports = serde_json::Map::new();
    for case in fixture["cases"].as_array().unwrap() {
        let mut arguments = case["input"].clone();
        replace(&mut arguments, "$ORIGIN", &server.origin);
        let mut policy = WebFetchPolicy::default();
        if case["private"] != false {
            policy.private_origins.push(server.origin.clone());
        }
        let tool = WebFetchTool::new()
            .with_limits(
                serde_json::from_value(case.get("limits").cloned().unwrap_or(json!({}))).unwrap(),
            )
            .unwrap()
            .with_policy(policy)
            .unwrap();
        let token = CancellationToken::new();
        if case["cancelled"] == true {
            token.cancel();
        }
        let result = tool
            .fetch(serde_json::from_value(arguments).unwrap(), &token)
            .await;
        let mut result = match result {
            Ok(value) => serde_json::to_value(value).unwrap(),
            Err(error) => json!({"error": match error {
                Error::InvalidInput(_) => "invalid_input", Error::Refused(_) => "refused",
                Error::LimitExceeded(_) => "limit", Error::Unsupported(_) => "unsupported",
                Error::Cancelled => "cancelled", Error::Io { .. } => "io", _ => "unknown",
            }}),
        };
        replace(&mut result, &server.origin, "$ORIGIN");
        for (pointer, expected) in case["expect"].as_object().unwrap() {
            assert_eq!(
                result.pointer(pointer),
                Some(expected),
                "{}: {result}",
                case["name"]
            );
        }
        reports.insert(case["name"].as_str().unwrap().into(), result);
    }
    assert!(!server.seen.lock().unwrap().is_empty());
    if let Ok(path) = std::env::var("ROOMKIT_WEB_RESULTS") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"fixture":fixture,"reports":reports})).unwrap(),
        )
        .unwrap();
    }
}
