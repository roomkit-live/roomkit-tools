# roomkit-tools-web

The `web_fetch` tool: an HTTP(S) GET with a total deadline, bounded
redirects and response size, returning the page as Markdown, text or HTML.
No JavaScript runs. By default only public origins are reachable: private,
loopback and link-local addresses are refused unless the host allows them
with a `WebFetchPolicy`.

## Features

None. HTTPS uses rustls with the bundled Mozilla roots.

## Example

```rust,no_run
use roomkit_tools_web::{WebFetchInput, WebFetchTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fetch = WebFetchTool::new();
    let page = fetch
        .fetch(WebFetchInput::new("https://www.rust-lang.org/"), &CancellationToken::new())
        .await?;
    println!("{} ({} redirects)", page.data.final_url, page.data.redirects);
    print!("{}", page.data.content); // Markdown by default
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
