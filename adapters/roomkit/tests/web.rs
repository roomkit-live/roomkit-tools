//! Independently selectable network tool, results and lifecycle cancellation.
#![expect(clippy::unwrap_used, reason = "test fixture failures must fail tests")]
#[path = "../../../tests/support/web.rs"]
mod web;
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_roomkit::WebFetchToolset;
use roomkit_tools_web::{WebFetchPolicy, WebFetchTool};
use serde_json::json;

fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("web").unwrap(),
        ChannelId::new("agent").unwrap(),
    )
}
fn call(name: &str, url: &str) -> ToolCall {
    ToolCall::new(
        "fetch",
        name,
        serde_json::from_value(json!({"url":url})).unwrap(),
    )
}

#[tokio::test]
async fn schema_result_policy_and_closing_preserve_native_contracts() {
    let server = web::Server::start().await;
    let mut policy = WebFetchPolicy::default();
    policy.allowed_origins = Some(vec![server.origin.clone()]);
    policy.private_origins = vec![server.origin.clone()];
    let tools = WebFetchToolset::new(WebFetchTool::new().with_policy(policy).unwrap());
    assert_eq!(tools.tools().len(), 1);
    assert_eq!(
        tools.tools()[0].parameters,
        WebFetchTool::definition().input_schema
    );
    assert!(matches!(
        tools
            .call(call("read", "https://example.com"), context())
            .await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(
        tools
            .call(call("web_fetch", "https://example.com"), context())
            .await,
        Err(ToolError::Refused(_))
    ));
    let ToolOutput::Json(result) = tools
        .call(
            call("web_fetch", &format!("{}/start", server.origin)),
            context(),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    assert_eq!(
        result["data"]["final_url"],
        format!("{}/doc", server.origin)
    );
    assert_eq!(result["data"]["redirects"], 1);
    assert_eq!(*server.seen.lock().unwrap(), ["/start", "/doc"]);
    let pending = tools.call(
        call("web_fetch", &format!("{}/slow", server.origin)),
        context(),
    );
    let closing = async {
        while server.seen.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
        tools.close().await;
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(pending, closing)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(ToolError::Failed(_))));
    tools.close().await;
    assert!(matches!(
        tools
            .call(call("web_fetch", &server.origin), context())
            .await,
        Err(ToolError::Refused(_))
    ));
}
