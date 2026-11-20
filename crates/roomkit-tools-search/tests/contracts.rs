//! External backend extensibility without any built-in provider feature.
use roomkit_tools_core::ToolError;
use roomkit_tools_search::{
    BackendOutput, SearchBackend, SearchFuture, SearchHit, SearchLimits, SearchPayload,
    WebSearchInput, WebSearchTool,
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
    fn search<'a>(&'a self, _: &'a WebSearchInput, _: &'a CancellationToken) -> SearchFuture<'a> {
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
            .search(WebSearchInput::new("question"), &cancel)
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
                .search(WebSearchInput::new("q"), &CancellationToken::new())
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
        tool.search(WebSearchInput::new("q"), &CancellationToken::new())
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
        .search(WebSearchInput::new("q"), &CancellationToken::new())
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
        serde_json::from_value::<WebSearchInput>(
            json!({"query":"q","endpoint":"https://example.com"})
        )
        .is_err()
    );
    assert!(
        tool.with_limits(serde_json::from_value(json!({"max_results":101})).unwrap())
            .is_err()
    );
}
fn hit(url: &str, title: Option<&str>, content: Option<&str>) -> SearchHit {
    SearchHit {
        url: url.into(),
        title: title.map(Into::into),
        content: content.map(Into::into),
        omitted_bytes: 0,
    }
}

#[expect(clippy::unwrap_used, reason = "test limits are valid")]
fn results_tool(results: Vec<SearchHit>, max_output_bytes: usize) -> WebSearchTool {
    WebSearchTool::new(Arc::new(Custom(BackendOutput::results(results))))
        .with_limits(SearchLimits {
            max_output_bytes,
            ..SearchLimits::default()
        })
        .unwrap()
}

#[tokio::test]
async fn results_read_as_numbered_lines_with_short_excerpts() {
    let long = format!("Line one.\n\n  Line two {}", "x".repeat(1000));
    let tool = results_tool(
        vec![
            hit(
                "https://example.com/guide",
                Some("Shipping\nguide"),
                Some(&long),
            ),
            hit("https://example.com/api", None, Some("Reference")),
            hit("https://example.com/empty", Some("Empty"), None),
        ],
        SearchLimits::default().max_output_bytes,
    );
    let out = tool
        .search(WebSearchInput::new("q"), &CancellationToken::new())
        .await
        .unwrap();
    let lines: Vec<&str> = out.text.lines().collect();
    assert_eq!(
        lines[..3],
        [
            "provider=custom, partial=false, omitted_results=0",
            "",
            "1. Shipping guide — https://example.com/guide",
        ]
    );
    let excerpt = lines[3].strip_prefix("   Line one. Line two xxx").unwrap();
    assert!(excerpt.ends_with("x …"), "{excerpt}");
    assert!(lines[3].len() <= 3 + 600 + " …".len(), "{}", lines[3].len());
    assert_eq!(
        lines[4..],
        [
            "2. https://example.com/api",
            "   Reference",
            "3. Empty — https://example.com/empty",
        ]
    );
    // The text is a summary: data keeps every excerpt whole.
    let SearchPayload::Results { results } = out.data.payload else {
        unreachable!()
    };
    assert_eq!(results[0].content.as_deref(), Some(long.as_str()));
    assert_eq!(results[0].omitted_bytes, 0);
    assert!(!out.data.partial);
}

/// Ten excerpts of 4 KiB of code, which JSON escapes heavily: what a
/// code question gets from Exa.
fn code_results() -> Vec<SearchHit> {
    let code = "To parse JSON use `serde_json::from_str::<Value>(\"{\\\"a\\\": 1}\")`, then match on \"a\".\n```rust\nlet v: Value = serde_json::from_str(r#\"{\"name\": \"x\"}\"#)?;\n```\n";
    let mut excerpt = code.repeat(4096 / code.len() + 1);
    excerpt.truncate(4096);
    (0..10)
        .map(|i| {
            hit(
                &format!("https://docs.example.com/guide/json-{i}"),
                Some(&format!("Parsing JSON in Rust, part {i}")),
                Some(&excerpt),
            )
        })
        .collect()
}

#[tokio::test]
async fn results_fit_the_output_budget_by_shortening_excerpts_first() {
    // roomkit-code's budget; ten results used to exceed it outright.
    let max = 48 * 1024;
    let mut input = WebSearchInput::new("parse json rust");
    input.limit = 10;
    let out = results_tool(code_results(), max)
        .search(input, &CancellationToken::new())
        .await
        .unwrap();
    assert!(serde_json::to_vec(&out).unwrap().len() <= max);
    assert!(out.data.partial);
    assert_eq!(out.data.omitted_results, 0);
    let SearchPayload::Results { results } = &out.data.payload else {
        unreachable!()
    };
    assert_eq!(results.len(), 10);
    for result in results {
        let kept = result.content.as_ref().unwrap().len();
        assert!(kept > 1024, "{kept}");
        assert_eq!(kept + result.omitted_bytes, 4096);
    }
    assert!(
        out.text
            .starts_with("provider=custom, partial=true, omitted_results=0\n")
    );
}

#[tokio::test]
async fn results_past_the_budget_at_empty_excerpts_are_dropped_from_the_end() {
    let mut input = WebSearchInput::new("parse json rust");
    input.limit = 10;
    let out = results_tool(code_results(), 1500)
        .search(input.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert!(serde_json::to_vec(&out).unwrap().len() <= 1500);
    let SearchPayload::Results { results } = &out.data.payload else {
        unreachable!()
    };
    assert!((1..10).contains(&results.len()), "{}", results.len());
    assert_eq!(out.data.omitted_results, 10 - results.len());
    assert_eq!(results[0].url, "https://docs.example.com/guide/json-0");
    for result in results {
        assert_eq!(result.content.as_deref(), Some(""));
        assert_eq!(result.omitted_bytes, 4096);
    }
    assert!(out.data.partial);
    // Even no result does not fit: the call fails rather than mislead.
    assert!(matches!(
        results_tool(code_results(), 100)
            .search(input, &CancellationToken::new())
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
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
    fn search<'a>(&'a self, _: &'a WebSearchInput, _: &'a CancellationToken) -> SearchFuture<'a> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn generic_deadline_and_parent_cancellation_cover_custom_backends() {
    let tool = WebSearchTool::new(Arc::new(Pending));
    let mut input = WebSearchInput::new("q");
    // A zero timeout is refused when the input is read (tests/timeouts.rs).
    input.timeout_ms = std::num::NonZeroU64::new(10);
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
    let (out, ()) = tokio::join!(tool.search(WebSearchInput::new("q"), &cancel), result);
    assert!(matches!(out, Err(ToolError::Cancelled)));
}
