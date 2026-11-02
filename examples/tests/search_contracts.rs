//! Compare complete Rust/Python search results against the same HTTP responses.
#![forbid(unsafe_code)]

#[path = "../../tests/support/search.rs"]
mod search;
use roomkit_tools_core::ToolError;
use roomkit_tools_search::{
    ExaSearch, OllamaSearch, OpenAISearch, SearchBackend, SearchHttpConfig, WebSearchTool,
};
use serde_json::{Value, json};
use std::sync::Arc;
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
async fn shared_search_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/search.json")).unwrap();
    let server = search::Server::start().await;
    let mut reports = serde_json::Map::new();
    for case in fixture["cases"].as_array().unwrap() {
        let mut arguments = case["input"].clone();
        replace(&mut arguments, "$ORIGIN", &server.origin);
        let mut config = SearchHttpConfig {
            endpoint: Some(format!(
                "{}{}",
                server.origin,
                case["path"].as_str().unwrap()
            )),
            private_origins: vec![server.origin.clone()],
            ..SearchHttpConfig::default()
        };
        if case["deny"] == true {
            config.allowed_origins = Some(vec![]);
        }
        if let Some(cap) = case["max_response_bytes"].as_u64() {
            config.max_response_bytes = usize::try_from(cap).unwrap();
        }
        let backend: Arc<dyn SearchBackend> = match case["backend"].as_str().unwrap() {
            "exa" => Arc::new(
                ExaSearch::new(config)
                    .and_then(|exa| exa.with_api_key("fixture-key"))
                    .unwrap(),
            ),
            "ollama" => Arc::new(OllamaSearch::new("fixture-key", config).unwrap()),
            "openai" => {
                let mut options = case.get("backend_options").cloned().unwrap_or(json!({}));
                options["model"] = json!("fixture-model");
                Arc::new(
                    OpenAISearch::new(
                        "fixture-key",
                        serde_json::from_value(options).unwrap(),
                        config,
                    )
                    .unwrap(),
                )
            }
            _ => panic!("unknown fixture backend"),
        };
        assert!(!format!("{backend:?}").contains("fixture-key"));
        let tool = WebSearchTool::new(backend)
            .with_limits(
                serde_json::from_value(case.get("limits").cloned().unwrap_or(json!({}))).unwrap(),
            )
            .unwrap();
        let token = CancellationToken::new();
        if case["cancelled"] == true {
            token.cancel();
        }
        let result = tool
            .search(serde_json::from_value(arguments).unwrap(), &token)
            .await;
        let mut result = match result {
            Ok(value) => serde_json::to_value(value).unwrap(),
            Err(error) => json!({"error": match error {
                ToolError::InvalidInput(_) => "invalid_input", ToolError::AccessRefused(_) => "refused",
                ToolError::LimitExceeded(_) => "limit", ToolError::UnsupportedResource(_) => "unsupported",
                ToolError::Cancelled => "cancelled", ToolError::Io { .. } => "io", _ => "unknown",
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
    assert_requests(&server.seen.lock().unwrap());
    if let Ok(path) = std::env::var("ROOMKIT_SEARCH_RESULTS") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"fixture":fixture,"reports":reports})).unwrap(),
        )
        .unwrap();
    }
}

#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test assertions on fixture requests"
)]
fn assert_requests(seen: &[Value]) {
    let ollama = seen.iter().find(|r| r["path"] == "/ollama").unwrap();
    assert_eq!(
        ollama["body"],
        json!({"query":"shipping policy","max_results":2})
    );
    assert!(
        ollama["headers"]
            .as_str()
            .unwrap()
            .contains("authorization: Bearer fixture-key")
    );
    let exa = seen.iter().find(|r| r["path"] == "/exa").unwrap();
    assert_eq!(
        exa["body"],
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"web_search_exa","arguments":{"query":"shipping policy","numResults":2}}})
    );
    assert!(
        exa["headers"]
            .as_str()
            .unwrap()
            .contains("x-api-key: fixture-key")
    );
    let openai = seen.iter().find(|r| r["path"] == "/openai/answer").unwrap();
    assert_eq!(openai["body"]["model"], "fixture-model");
    assert_eq!(openai["body"]["input"], "shipping policy");
    assert_eq!(openai["body"]["store"], false);
    assert_eq!(openai["body"]["background"], false);
    assert_eq!(openai["body"]["stream"], false);
    assert_eq!(openai["body"]["max_tool_calls"], 1);
    assert_eq!(openai["body"]["max_output_tokens"], 2048);
    assert_eq!(openai["body"]["tool_choice"], "required");
    assert_eq!(
        openai["body"]["include"],
        json!(["web_search_call.action.sources"])
    );
    assert_eq!(
        openai["body"]["tools"],
        json!([{"type":"web_search","search_context_size":"low"}])
    );
    assert!(
        openai["headers"]
            .as_str()
            .unwrap()
            .contains("authorization: Bearer fixture-key")
    );
    assert!(openai["body"].get("previous_response_id").is_none());
}
