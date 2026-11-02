//! Independently selectable network tool, results and dropped calls.
#![expect(clippy::unwrap_used, reason = "test fixture failures must fail tests")]
#[path = "../../../tests/support/web.rs"]
mod web;
use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_roomkit::WebFetchToolset;
use roomkit_tools_web::{WebFetchPolicy, WebFetchTool};
use serde_json::json;

fn url(url: &str) -> Metadata {
    serde_json::from_value(json!({"url":url})).unwrap()
}

#[tokio::test]
async fn schema_result_policy_and_dropped_calls_preserve_native_contracts() {
    let server = web::Server::start().await;
    let mut policy = WebFetchPolicy::default();
    policy.allowed_origins = Some(vec![server.origin.clone()]);
    policy.private_origins = vec![server.origin.clone()];
    let tools = WebFetchToolset::new(WebFetchTool::new().with_policy(policy).unwrap());
    assert_eq!(tools.definition().name, "web_fetch");
    assert_eq!(
        tools.definition().parameters,
        WebFetchTool::definition().input_schema
    );
    assert!(matches!(
        tools
            .handler("read".into(), url("https://example.com"))
            .await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert!(matches!(
        tools
            .handler("web_fetch".into(), url("https://example.com"))
            .await,
        Err(ToolError::Refused(_))
    ));
    let start = format!("{}/start", server.origin);
    let ToolResult::Json(result) = tools
        .handler("web_fetch".into(), url(&start))
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
    // RoomKit cancels a call by dropping it; the adapter keeps serving.
    let mut pending = tools.handler("web_fetch".into(), url(&format!("{}/slow", server.origin)));
    let answered = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while server.seen.lock().unwrap().len() < 3 {
            tokio::select! {
                answer = &mut pending => return Some(answer),
                () = tokio::task::yield_now() => {}
            }
        }
        None
    })
    .await
    .unwrap();
    assert!(answered.is_none(), "the slow fetch answered: {answered:?}");
    drop(pending);
    assert!(matches!(
        tools.handler("web_fetch".into(), url(&start)).await,
        Ok(ToolResult::Json(_))
    ));
    assert_eq!(
        *server.seen.lock().unwrap(),
        ["/start", "/doc", "/slow", "/start", "/doc"]
    );
}
