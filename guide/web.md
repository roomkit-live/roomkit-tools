# Web tools

## Fetching a URL

Python snippets with `await` belong inside an async function. RoomKit fragments
assume an existing configured provider.

`web_fetch` makes HTTP(S) GET requests and returns Markdown by default, visible
text with `format="text"`, or the source with `format="html"`. HTML conversion
preserves code and resolves links against the final URL. Script/style/hidden
subtrees are omitted from Markdown/text. There is no JavaScript execution,
browser session or web search. Non-HTML text is preserved in every format.
The implementation supports UTF-8/ASCII textual MIME types and gzip. Other
encodings and binary resources produce an explicit unsupported-content error.

```python
from roomkit import AIChannel
from roomkit_tools import WebFetch

# Add just this capability to an existing RoomKit agent.
agent = AIChannel("agent", provider=provider, tools=[WebFetch()])

# Standalone async Python, with the same native implementation:
from roomkit_tools import WebFetchTool
result = await WebFetchTool().fetch("https://example.com", format="markdown")
print(result["data"]["final_url"], result["data"]["http_ok"])
print(result["text"])
```

The host can restrict `allowed_origins` to exact scheme/host/effective-port
origins. `None` allows public destinations; `[]` denies all. Private, loopback,
link-local and reserved addresses are refused unless the host explicitly adds
the exact origin to `private_origins`; this never bypasses `allowed_origins`.
Each redirect repeats origin and DNS checks. Validated IP addresses are pinned
for the connection while TLS checks the original hostname. Ambient proxies,
cookies and URL credentials are disabled. TLS uses the ring provider and the
bundled Mozilla root certificates, the same on every host: the system
certificate store is not consulted, so a private certificate authority is not
trusted. No custom authentication/header API is provided. These controls apply to this tool, not other selected capabilities.

Default bounds: 8 KiB URL, 5 MiB decoded body, 5 MiB converted text, 50 KiB
returned content, 10 redirects, 30-second total deadline (host maximum 120
seconds). The deadline includes DNS, redirects, downloads and conversion.
Hosts configure these through `WebFetchLimits` or Python constructor keywords.
An oversized download is refused, including streaming/decompressed responses;
a preview is clipped only after successful bounded ingestion and conversion.
`response_bytes`, `content_bytes`, `omitted_bytes` and `truncated` distinguish
these stages. A clipped preview has no archive or continuation cursor.
`status`, `http_ok` and `final_url` describe the actual final HTTP response;
non-2xx responses remain data if their body is supported. Transport failures,
refused destinations, limits and unsupported content are typed tool errors.

HTML conversion runs outside the async executor with two slots per tool (shared
by clones). The parser reads the page in 1 KiB chunks and stops building the
DOM once it holds more than 100,000 nodes or nests deeper than 128 levels,
counting the elements HTML parsing recreates for unclosed formatting tags and
the attributes repeated `<html>` or `<body>` tags merge (each merge counts the
attributes it is compared with); the page is then refused. The finished DOM is
walked again with the same node and depth limits, and expanded link bytes are
bounded. The parser compares each attribute of a tag with the earlier ones of
that tag before any limit sees it, so a tag with tens of thousands of
attributes takes seconds; only cancellation and the deadline stop that work.

Markdown can be much larger than the page: a pipe table pads every row to the
widest cell of each column and to the widest row, and each enclosing quote or
list item prefixes every line inside it again. Code and math spans keep their
whitespace and line breaks, so their text is padded and prefixed as written.
The same walk therefore computes an upper bound of the Markdown size from the
DOM, counting that padding and those prefixes, and refuses the page with the
converted-text limit error before any Markdown is written when the bound exceeds
`max_converted_bytes`. The bound follows the converter's choices: only a table
with header cells that is not inside another table's cell becomes a padded pipe
table; layout tables count as their plain content. The bound only overestimates,
by up to about two times on ordinary pages, so a page whose Markdown would fall
just under the limit can be refused. The rendered Markdown
is checked against the limit again. These are work/output bounds, not strict
peak memory limits. Cancellation and the deadline drop network work and stop
parsing at the next chunk; issued DNS and Markdown rendering already under way
may finish later. Fetched content remains untrusted reference data.

Run `python examples/roomkit_web.py` for a self-checking local demonstration,
or add `--live` with `OPENAI_BASE_URL` and `OPENAI_MODEL` for vLLM. The local
server supplies sample HTML only; HTTP fetching and conversion execute in Rust.

## Modular web search

Choose a search backend independently of your agent model. The Rust tool is
`WebSearchTool::new(Arc<dyn SearchBackend>)`. Implement `SearchBackend` in another
crate to add a service; there is no closed provider registry. Built-in backends
are enabled with Cargo features `exa`, `ollama` and/or `openai`. The RoomKit Rust adapter
accepts the generic tool without enabling a provider feature itself.

For RoomKit Python:

```python
from roomkit import AIChannel
from roomkit_tools import WebSearch, WebFetch
from roomkit_tools.search import ExaSearch, OllamaSearch

agent = AIChannel(
    "agent",
    provider=vllm_provider,  # Your existing model/provider.
    tools=[WebSearch(backend=ExaSearch()), WebFetch()],
)
# Or select the hosted Ollama search API with an explicit account key:
# tools=[WebSearch(backend=OllamaSearch(api_key=search_key))]
```

`ExaSearch` uses [Exa's hosted MCP endpoint](https://exa.ai/mcp) with optional `api_key`; anonymous
access is subject to service limits. `OllamaSearch` uses the [hosted Ollama REST
API](https://docs.ollama.com/capabilities/web-search) and requires an explicit account key. A local Ollama installation does not
provide this web index. No backend reads keys from environment variables.
No search server or second local model needs to be started.

Without RoomKit, use `WebSearchTool(backend=...)` and
`await tool.search("query", limit=5, timeout_ms=30000)`. It returns the same
`data`/`text` envelope as Rust. `backend.capabilities` exposes supported result
kinds and service limits. A new native backend needs a binding factory to be
available in Python; there is no Python HTTP fallback.

Input is only `query`, `limit` and `timeout_ms`. The host selects credentials,
endpoint, origin policy and budgets. Exa and Ollama return
`data.payload.kind == "results"` with ranked URL/title/excerpt entries.
`partial`, `omitted_results` and per-excerpt `omitted_bytes` expose known
omissions. Zero results is a valid response, distinct from HTTP/quota failures,
MCP errors and unsupported formats. Exa uses a bounded parser for its current
text blocks over JSON or SSE; an upstream format change can require an adapter
update. This is not a general MCP client.

Exa's MCP service returns all hits as one formatted text block: `Title:` and
`URL:` lines, then metadata and page excerpts, with a Markdown rule (`---`)
between records. There is no structured form on that endpoint, so hit
boundaries, titles and URLs are parsed from provider text that includes page
content. A rule starts a new record only when a `Title:` line then a `URL:`
line follow it; any other rule stays inside the preceding excerpt, so an
ordinary Markdown rule does not fail the search. A record whose URL is not a
usable HTTP(S) source (not HTTP(S), with credentials, or over 8 KiB) is dropped
with its excerpt; a notice says so and `partial` is set. A page can still
imitate a complete record and have its own text attributed to another URL, or
dropped. When Exa's text holds more records than requested, a notice says so
and `partial` is set.
Treat Exa titles and URLs as claims from the page text and confirm with
`web_fetch` before attributing content to a URL.

The contract also supports `answer` with sources and UTF-8 citation offsets,
and `not_performed` when a model did not search. Common validation checks
source indexes and character boundaries; oversized answers fail without
clipping citations. Optional `usage` contains provider-reported counters only.
`OpenAISearch` implements these shapes through a separate
[Responses web-search request](https://developers.openai.com/api/docs/guides/tools-web-search):

```python
from roomkit_tools import WebSearch
from roomkit_tools.search import OpenAISearch

tools = [WebSearch(backend=OpenAISearch(
    api_key=search_key,
    model=search_model,          # Explicit model supporting Responses web search.
    max_tool_calls=1,
    max_output_tokens=2048,
))]
```

The model performs a required web search and generates an answer. The adapter
only returns `answer` after observing a completed `search` action. A completed
response without one becomes `not_performed`; it does not forward an unverified
answer as web evidence. Failed/unfinished tool calls and malformed payloads
raise errors. Incomplete responses with a completed search and usable text are
marked `partial`; missing URL citations are also reported explicitly.

One synchronous request uses `store=false`, no history and no streaming,
background execution, retry or continuation. `max_tool_calls` (1–16) and
`max_output_tokens` (1–1,000,000) are request ceilings, not monetary budgets.
A compatible model and funded API access are the host's responsibility; the
library never selects a model or reads environment variables. Local cancellation
stops waiting but does not guarantee remote computation or billing has stopped.
No OpenAI SDK dependency is required.

URL annotations use Unicode character indexes, converted to UTF-8 byte offsets
before concatenating text parts; `data.payload.answer` keeps the original text.
The `text` rendition inserts visible Markdown citation links. Hosts displaying
answers should preserve clickable citations. `limit` bounds retained sources:
all cited sources are kept, uncited extras are omitted with a count, and a limit
below the number of cited sources fails explicitly. Citation source indexes are
remapped when uncited sources are omitted. No generated excerpt is invented.
`usage.search_requests` counts observed search actions, while token counters are
copied from the provider when present; absent token usage remains unknown.

**Anthropic, Gemini and SearXNG backends are not implemented yet.** Search inside
the main AI generation remains a separate RoomKit provider integration. OpenAI
protocol handling is covered by local response fixtures; these automated tests
do not establish account/model availability or replace a live API validation.

Defaults: 4096 query bytes, at most 10 requested results, 4096 excerpt bytes per
source, 128 KiB complete serialized output, 30 s default / 120 s maximum total
deadline. Each built-in backend caps decoded HTTP input at 512 KiB (host maximum
16 MiB) and shares two blocking decoding slots. Network I/O is cancelled when
the call ends; issued DNS and bounded decoding may finish later. These are work
and output bounds, not a hard peak-memory guarantee.

Built-in transports share `web_fetch`'s origin/DNS rules, disable inherited
proxies and refuse redirects to avoid forwarding credentials or queries.
Endpoints require HTTPS unless the host explicitly permits a private origin.
Response error bodies and credentials are not included in public error
messages. There are no automatic retries, service fallback or implicit page
fetches. Pages, excerpts and provider notices remain untrusted reference data.

## Examples

See the [examples index](../examples/README.md#web-fetch-and-search) for offline
fixtures and opt-in live providers. Search credentials belong to the host, and
are separate from the main agent model's configuration.

[Documentation index](README.md) · [Project overview](../README.md)
