//! Explicit backend selection: deterministic local HTTP by default, Exa with --live.
#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "standalone demonstration output")]
use roomkit_tools_search::{
    ExaSearch, OllamaSearch, SearchBackend, SearchHttpConfig, SearchInput, SearchPayload,
    WebSearchTool,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let live = std::env::args().any(|arg| arg == "--live");
    let backend: Arc<dyn SearchBackend> = if live {
        Arc::new(ExaSearch::new(None, SearchHttpConfig::default())?)
    } else {
        // A tiny fixture service for an offline example; HTTP and parsing remain in Rust.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(stream.read_u8().await?);
            }
            let text = String::from_utf8_lossy(&header);
            let size = text
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0; size];
            stream.read_exact(&mut body).await?;
            let body = r#"{"results":[{"title":"Serde guide","url":"https://serde.rs/","content":"Serialization framework."}]}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
            Ok::<_, std::io::Error>(())
        });
        let mut config = SearchHttpConfig::default();
        config.endpoint = Some(format!("{origin}/search"));
        config.policy.private_origins.push(origin);
        Arc::new(OllamaSearch::new("fixture-only", config)?)
    };
    let tool = WebSearchTool::new(backend);
    let output = tool
        .search(
            SearchInput::new("Serde Rust official documentation"),
            &CancellationToken::new(),
        )
        .await?;
    if !live {
        let SearchPayload::Results { results } = &output.data.response.payload else {
            return Err("expected results".into());
        };
        assert_eq!(
            results.first().map(|r| r.url.as_str()),
            Some("https://serde.rs/")
        );
    }
    println!("{}", output.text);
    Ok(())
}
