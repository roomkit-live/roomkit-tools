//! Generic search adapter preserves provider output and cancels dropped calls.
#![expect(clippy::unwrap_used, reason = "test assertions")]
use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_roomkit::WebSearch;
use roomkit_tools_search::{
    BackendOutput, SearchBackend, SearchFuture, WebSearchInput, WebSearchTool,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
/// A backend whose `wait` query never ends, keeping the token it was given.
#[derive(Debug, Default)]
struct Backend {
    waiting: Arc<Mutex<Option<CancellationToken>>>,
}
impl SearchBackend for Backend {
    fn capabilities(&self) -> roomkit_tools_search::SearchCapabilities {
        roomkit_tools_search::SearchCapabilities::direct(10)
    }
    fn name(&self) -> &'static str {
        "custom"
    }
    fn search<'a>(
        &'a self,
        input: &'a WebSearchInput,
        cancel: &'a CancellationToken,
    ) -> SearchFuture<'a> {
        Box::pin(async move {
            if input.query == "wait" {
                *self.waiting.lock().unwrap() = Some(cancel.clone());
                std::future::pending::<()>().await;
            }
            Ok(BackendOutput::results(vec![]))
        })
    }
}
fn query(query: &str) -> Metadata {
    serde_json::from_value(json!({"query":query})).unwrap()
}
#[tokio::test]
async fn search_is_independently_selectable_and_dropping_a_call_cancels_it() {
    let backend = Backend::default();
    let waiting = Arc::clone(&backend.waiting);
    let tools = WebSearch::new(WebSearchTool::new(Arc::new(backend)));
    assert_eq!(tools.definition().name, "web_search");
    assert_eq!(
        tools.definition().parameters,
        WebSearchTool::definition().input_schema
    );
    assert!(matches!(
        tools.handler("web_search".into(), query("")).await,
        Err(ToolError::Refused(_))
    ));
    let ToolResult::Json(result) = tools
        .handler("web_search".into(), query("query"))
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    assert_eq!(result["data"]["provider"], "custom");
    assert_eq!(result["data"]["payload"]["results"], json!([]));
    // RoomKit cancels a call by dropping it: the backend's token is cancelled.
    let mut pending = tools.handler("web_search".into(), query("wait"));
    let token = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(token) = waiting.lock().unwrap().clone() {
                return Ok(token);
            }
            tokio::select! {
                answer = &mut pending => return Err(answer),
                () = tokio::task::yield_now() => {}
            }
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert!(!token.is_cancelled());
    drop(pending);
    assert!(token.is_cancelled());
}

#[path = "../../../tests/support/search.rs"]
mod http;

#[tokio::test]
async fn native_openai_answer_preserves_citations_and_usage_through_roomkit() {
    let server = http::Server::start().await;
    let config = roomkit_tools_search::SearchHttpConfig {
        endpoint: Some(format!("{}/openai/answer", server.origin)),
        private_origins: vec![server.origin.clone()],
        ..roomkit_tools_search::SearchHttpConfig::default()
    };
    let backend = roomkit_tools_search::OpenAISearch::new(
        "fixture-key",
        roomkit_tools_search::OpenAISearchConfig {
            max_tool_calls: 1,
            ..roomkit_tools_search::OpenAISearchConfig::new("fixture-model")
        },
        config,
    )
    .unwrap();
    let tools = WebSearch::new(WebSearchTool::new(Arc::new(backend)));
    let ToolResult::Json(result) = tools
        .handler("web_search".into(), query("query"))
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    assert_eq!(result["data"]["payload"]["kind"], "answer");
    assert_eq!(result["data"]["payload"]["citations"][0]["end"], 9);
    assert_eq!(result["data"]["usage"]["search_requests"], 1);
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("[source 1](<https://example.com/guide>)")
    );
    assert_eq!(server.seen.lock().unwrap().len(), 1);
}
