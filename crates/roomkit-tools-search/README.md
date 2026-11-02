# roomkit-tools-search

The `web_search` tool over a backend the host selects. A backend returns
ranked documents, a cited answer, or an explicit report that no search ran;
every result is validated and bounded the same way. Hosts can implement the
`SearchBackend` trait, or enable a packaged one.

## Features

All off by default; enable only the backends the host uses.

- `exa`: `ExaSearch`, Exa's hosted search endpoint, anonymous or with an API
  key.
- `ollama`: `OllamaSearch`, Ollama's hosted search API.
- `openai`: `OpenAISearch`, a cited answer from an OpenAI Responses model with
  web search, separate from the agent's own model.

## Example

With `features = ["exa"]`:

```rust,no_run
use std::sync::Arc;

use roomkit_tools_search::{ExaSearch, SearchHttpConfig, SearchInput, WebSearchTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Exa's limited anonymous service; `with_api_key` adds a key.
    let backend = ExaSearch::new(SearchHttpConfig::default())?;
    let search = WebSearchTool::new(Arc::new(backend));
    let result = search
        .search(SearchInput::new("tokio CancellationToken"), &CancellationToken::new())
        .await?;
    print!("{}", result.text);
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
