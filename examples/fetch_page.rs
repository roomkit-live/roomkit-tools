//! Fetch a local page through a redirect and verify the actual Rust result.
#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "example prints verified fetch result")]
use roomkit_tools_web::{WebFetchInput, WebFetchPolicy, WebFetchTool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        for body in [
            "HTTP/1.1 302 Found\r\nLocation: /guide\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<h1>Shipping</h1><p>Free from <strong>50 units</strong>.</p>",
        ] {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await?);
            }
            stream.write_all(body.as_bytes()).await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let mut policy = WebFetchPolicy::default();
    policy.allowed_origins = Some(vec![origin.clone()]);
    policy.private_origins = vec![origin.clone()];
    let result = WebFetchTool::new()
        .with_policy(policy)?
        .fetch(
            WebFetchInput::new(format!("{origin}/start")),
            &CancellationToken::new(),
        )
        .await?;
    server.await??;
    assert_eq!(result.data.final_url, format!("{origin}/guide"));
    assert_eq!(result.data.redirects, 1);
    assert!(result.data.http_ok && !result.data.truncated);
    assert!(result.data.content.contains("**50 units**"));
    println!("{}", result.text);
    Ok(())
}
