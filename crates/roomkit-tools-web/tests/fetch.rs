//! Real network requests verify bounds, access scope and cancellation.
#[path = "../../../tests/support/web.rs"]
mod web;
use roomkit_tools_core::Error;
use roomkit_tools_web::{WebFetchInput, WebFetchLimits, WebFetchPolicy, WebFetchTool};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "valid test origin")]
fn local(server: &web::Server) -> WebFetchTool {
    let mut policy = WebFetchPolicy::default();
    policy.allowed_origins = Some(vec![server.origin.clone()]);
    policy.private_origins = vec![server.origin.clone()];
    WebFetchTool::new().with_policy(policy).unwrap()
}

#[tokio::test]
async fn redirects_conversion_code_and_links_use_the_final_url() {
    let server = web::Server::start().await;
    let result = local(&server)
        .fetch(
            WebFetchInput::new(format!("{}/start#ignored", server.origin)),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.data.final_url, format!("{}/doc", server.origin));
    assert_eq!(result.data.redirects, 1);
    assert!(result.data.content.contains("# Shipping API"));
    assert!(
        result
            .data
            .content
            .contains("```python\nshipping_fee(50) == 0")
    );
    assert!(
        result
            .data
            .content
            .contains(&format!("[Guide]({}/guide)", server.origin))
    );
    assert!(!result.data.content.contains("SECRET"));
    assert!(!result.data.content.contains("HIDDEN"));
    assert_eq!(*server.seen.lock().unwrap(), ["/start", "/doc"]);
}

#[tokio::test]
async fn policy_and_invalid_input_refuse_before_network_effects() {
    let server = web::Server::start().await;
    let tool = local(&server);
    let cancel = CancellationToken::new();
    for url in [
        "file:///etc/passwd",
        "ftp://example.com",
        "not a URL",
        "http://user:password@127.0.0.1/",
    ] {
        assert!(tool.fetch(WebFetchInput::new(url), &cancel).await.is_err());
    }
    assert!(
        WebFetchTool::new()
            .fetch(
                WebFetchInput::new(format!("{}/doc", server.origin)),
                &cancel
            )
            .await
            .is_err()
    );
    let mut policy = WebFetchPolicy::default();
    policy.allowed_origins = Some(Vec::new());
    policy.private_origins.push(server.origin.clone());
    assert!(
        WebFetchTool::new()
            .with_policy(policy)
            .unwrap()
            .fetch(
                WebFetchInput::new(format!("{}/doc", server.origin)),
                &cancel
            )
            .await
            .is_err()
    );
    cancel.cancel();
    assert!(matches!(
        tool.fetch(
            WebFetchInput::new(format!("{}/doc", server.origin)),
            &cancel
        )
        .await,
        Err(Error::Cancelled)
    ));
    assert!(server.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn blocked_redirect_does_not_reach_the_target() {
    let server = web::Server::start().await;
    assert!(matches!(
        local(&server)
            .fetch(
                WebFetchInput::new(format!("{}/blocked", server.origin)),
                &CancellationToken::new()
            )
            .await,
        Err(Error::Refused(_))
    ));
    assert_eq!(*server.seen.lock().unwrap(), ["/blocked"]);
}

#[tokio::test]
async fn cancellation_interrupts_a_body_read_and_another_call_still_works() {
    let server = web::Server::start().await;
    let tool = local(&server);
    let token = CancellationToken::new();
    let task_tool = tool.clone();
    let task_token = token.clone();
    let url = format!("{}/slow", server.origin);
    let task =
        tokio::spawn(async move { task_tool.fetch(WebFetchInput::new(url), &task_token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.seen.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    token.cancel();
    assert!(matches!(task.await.unwrap(), Err(Error::Cancelled)));
    assert!(
        tool.fetch(
            WebFetchInput::new(format!("{}/json", server.origin)),
            &CancellationToken::new()
        )
        .await
        .is_ok()
    );
}

#[test]
fn configuration_and_schemas_have_no_model_access_controls() {
    let schema = WebFetchTool::definition().input_schema;
    assert!(schema["properties"].get("private_origins").is_none());
    assert_eq!(schema["additionalProperties"], false);
    let mut limits = WebFetchLimits::default();
    limits.max_response_bytes = 0;
    assert!(WebFetchTool::new().with_limits(limits).is_err());
    let mut policy = WebFetchPolicy::default();
    policy.private_origins = vec!["http://localhost/path".into()];
    assert!(WebFetchTool::new().with_policy(policy).is_err());
}
