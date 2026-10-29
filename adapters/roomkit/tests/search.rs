//! Generic search adapter preserves provider output and lifecycle cancellation.
#![expect(clippy::unwrap_used, reason = "test assertions")]
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_roomkit::WebSearchToolset;
use roomkit_tools_search::{
    BackendOutput, SearchBackend, SearchFuture, SearchInput, WebSearchTool,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
#[derive(Debug)]
struct Backend;
impl SearchBackend for Backend {
    fn capabilities(&self) -> roomkit_tools_search::SearchCapabilities {
        roomkit_tools_search::SearchCapabilities::direct(10)
    }
    fn name(&self) -> &'static str {
        "custom"
    }
    fn search<'a>(&'a self, input: &'a SearchInput, _: &'a CancellationToken) -> SearchFuture<'a> {
        Box::pin(async move {
            if input.query == "wait" {
                std::future::pending::<()>().await;
            }
            Ok(BackendOutput::results(vec![]))
        })
    }
}
fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("search").unwrap(),
        ChannelId::new("agent").unwrap(),
    )
}
fn call(query: &str) -> ToolCall {
    ToolCall::new(
        "search",
        "web_search",
        serde_json::from_value(json!({"query":query})).unwrap(),
    )
}
#[tokio::test]
async fn search_is_independently_selectable_and_closing_cancels() {
    let tools = WebSearchToolset::new(WebSearchTool::new(Arc::new(Backend)));
    assert_eq!(tools.tools().len(), 1);
    assert_eq!(
        tools.tools()[0].parameters,
        WebSearchTool::definition().input_schema
    );
    assert!(matches!(
        tools.call(call(""), context()).await,
        Err(ToolError::Refused(_))
    ));
    let ToolOutput::Json(result) = tools.call(call("query"), context()).await.unwrap() else {
        panic!("expected JSON")
    };
    assert_eq!(result["data"]["provider"], "custom");
    assert_eq!(result["data"]["payload"]["results"], json!([]));
    let closing = async {
        tokio::task::yield_now().await;
        tools.close().await;
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(tools.call(call("wait"), context()), closing)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(ToolError::Failed(_))));
    assert!(matches!(
        tools.call(call("query"), context()).await,
        Err(ToolError::Refused(_))
    ));
}
