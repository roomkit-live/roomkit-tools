//! External backend extensibility without any built-in provider feature.
use roomkit_tools_core::ToolError;
use roomkit_tools_search::{
    BackendOutput, SearchBackend, SearchFuture, SearchHit, SearchInput, SearchLimits,
    SearchPayload, WebSearchTool,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Custom(BackendOutput);
#[expect(clippy::unwrap_used, reason = "test fixture")]
impl SearchBackend for Custom {
    fn capabilities(&self) -> roomkit_tools_search::SearchCapabilities {
        serde_json::from_value(
            json!({"result_kinds":["results","answer","not_performed"],"max_results":null}),
        )
        .unwrap()
    }
    fn name(&self) -> &'static str {
        "custom"
    }
    fn search<'a>(&'a self, _: &'a SearchInput, _: &'a CancellationToken) -> SearchFuture<'a> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}
#[expect(clippy::unwrap_used, reason = "test fixture must decode")]
fn backend(value: serde_json::Value) -> WebSearchTool {
    WebSearchTool::new(Arc::new(Custom(serde_json::from_value(value).unwrap())))
}
fn answer() -> serde_json::Value {
    json!({"payload":{"kind":"answer","answer":"café ships free", "sources":[{"url":"https://example.com","title":"Guide","content":null}], "citations":[{"source_index":0,"start":0,"end":5}]}, "notices":["Provider presentation metadata"]})
}
#[tokio::test]
async fn cited_answer_and_non_execution_remain_distinct_from_empty_results() {
    let cancel = CancellationToken::new();
    for value in [
        answer(),
        json!({"payload":{"kind":"not_performed","reason":"Search was not invoked"}}),
        json!({"payload":{"kind":"results","results":[]}}),
    ] {
        let result = backend(value.clone())
            .search(SearchInput::new("question"), &cancel)
            .await
            .unwrap();
        let data = serde_json::to_value(result.data).unwrap();
        assert_eq!(data["payload"]["kind"], value["payload"]["kind"]);
        if value["payload"]["kind"] == "answer" {
            assert_eq!(data["payload"]["citations"], value["payload"]["citations"]);
            assert_eq!(data["notices"], value["notices"]);
        }
    }
}
#[tokio::test]
async fn invalid_citations_fail_instead_of_fabricating_or_reindexing() {
    for (key, bad) in [
        ("start", json!(4)),
        ("end", json!(4)),
        ("end", json!(999)),
        ("source_index", json!(1)),
    ] {
        let mut value = answer();
        value["payload"]["citations"][0][key] = bad;
        assert!(matches!(
            backend(value)
                .search(SearchInput::new("q"), &CancellationToken::new())
                .await,
            Err(ToolError::UnsupportedResource(_))
        ));
    }
}
#[tokio::test]
async fn answer_bounds_do_not_clip_citations() {
    let tool = backend(answer())
        .with_limits(SearchLimits {
            max_output_bytes: 12,
            ..SearchLimits::default()
        })
        .unwrap();
    assert!(matches!(
        tool.search(SearchInput::new("q"), &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
}
#[tokio::test]
async fn utf8_excerpt_clipping_is_accounted_for() {
    let tool = WebSearchTool::new(Arc::new(Custom(BackendOutput::results(vec![SearchHit {
        url: "https://example.com".into(),
        title: None,
        content: Some("éé".into()),
        omitted_bytes: 1,
    }]))))
    .with_limits(SearchLimits {
        max_content_bytes: 3,
        ..SearchLimits::default()
    })
    .unwrap();
    let out = tool
        .search(SearchInput::new("q"), &CancellationToken::new())
        .await
        .unwrap();
    let SearchPayload::Results { results } = out.data.payload else {
        unreachable!()
    };
    assert_eq!(results[0].content.as_deref(), Some("é"));
    assert_eq!(results[0].omitted_bytes, 3);
    assert!(out.data.partial);
}
#[tokio::test]
async fn arguments_and_limits_are_host_controlled() {
    let tool = backend(json!({"payload":{"kind":"results","results":[]}}));
    for input in [
        json!({"query":""}),
        json!({"query":"q","limit":0}),
        json!({"query":"q","timeout_ms":0}),
        json!({"query":"q","limit":11}),
    ] {
        assert!(matches!(
            tool.search(
                serde_json::from_value(input).unwrap(),
                &CancellationToken::new()
            )
            .await,
            Err(ToolError::InvalidInput(_))
        ));
    }
    assert!(
        serde_json::from_value::<SearchInput>(
            json!({"query":"q","endpoint":"https://example.com"})
        )
        .is_err()
    );
    assert!(
        tool.with_limits(serde_json::from_value(json!({"max_results":101})).unwrap())
            .is_err()
    );
}
#[derive(Debug)]
struct Pending;
#[expect(clippy::unwrap_used, reason = "test fixture")]
impl SearchBackend for Pending {
    fn capabilities(&self) -> roomkit_tools_search::SearchCapabilities {
        serde_json::from_value(
            json!({"result_kinds":["results","answer","not_performed"],"max_results":null}),
        )
        .unwrap()
    }
    fn name(&self) -> &'static str {
        "pending"
    }
    fn search<'a>(&'a self, _: &'a SearchInput, _: &'a CancellationToken) -> SearchFuture<'a> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn generic_deadline_and_parent_cancellation_cover_custom_backends() {
    let tool = WebSearchTool::new(Arc::new(Pending));
    let mut input = SearchInput::new("q");
    input.timeout_ms = Some(10);
    assert!(matches!(
        tool.search(input, &CancellationToken::new()).await,
        Err(ToolError::LimitExceeded(_))
    ));
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let result = async {
        tokio::task::yield_now().await;
        child.cancel();
    };
    let (out, ()) = tokio::join!(tool.search(SearchInput::new("q"), &cancel), result);
    assert!(matches!(out, Err(ToolError::Cancelled)));
}
