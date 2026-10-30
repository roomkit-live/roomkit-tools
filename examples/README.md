# Examples

Run commands below from the repository root, with the package installed in the
active Python environment. See [installation](../guide/installation.md).
Examples use temporary files and clean them up; shell examples still run with
host permissions.

## Start small

| Example | Command | What it demonstrates |
| --- | --- | --- |
| Standalone filesystem | `python examples/discover_files.py` | List, glob, grep and read with actual Rust tools |
| RoomKit read | `python examples/roomkit_read.py` | A full RoomKit conversation with one selected tool |
| Delegated read | `python examples/roomkit_delegate.py` | A principal delegates to a researcher and receives its report |
| Image viewing | `python examples/roomkit_image.py` | Actual image parts reach the model context |

Standalone examples need only `roomkit-tools`. RoomKit examples need the
`[roomkit]` extra. Their default providers are scripted: tools and RoomKit run
for real, but model decisions are predetermined. Default runs need no external
service or credentials. The image example's scripted mode checks transport,
not visual recognition.

## Standalone Python

| Command | Scope |
| --- | --- |
| `python examples/edit_file.py` | Create, read, revise and reject a stale edit |
| `python examples/patch_files.py` | Compare, preview and apply structured changes |
| `python examples/run_shell.py` | Foreground execution, stdout/stderr and status |
| `python examples/run_process.py` | Persistent stdin, exact bytes, EOF and cleanup |

## Use a live model

For examples exposing `--live`, install the vLLM provider extra and configure an
OpenAI-compatible server with tool calling enabled:

```sh
python -m pip install 'roomkit[vllm]>=0.110,<0.112'
export OPENAI_BASE_URL=http://localhost:8000/v1
export OPENAI_MODEL=your-served-model
# Set OPENAI_API_KEY in your environment if your server requires authentication.
python examples/roomkit_read.py --live
```

The provider and model belong to the host. The native tool implementation is the
same in scripted and live modes. Image viewing requires a model accepting images:

```sh
python examples/roomkit_image.py --live
```

The image contents are not included in the prompt. Compare the model's answer
with the reference printed afterward. Live-model claims alone do not validate a
tool call; examples inspect actual tool results.

## Coding and output workflows

| Command | Verification |
| --- | --- |
| `python examples/roomkit_code.py` | Reproduce a failing test, grep/read/edit/diff, then verify the correction independently |
| `python examples/roomkit_outline.py` | Locate a method, read its range, edit with its revision and rerun tests |
| `python examples/roomkit_output.py` | Retrieve an omitted diagnostic from an archived log without rerunning the command |

Add `--live` for a configured model. `roomkit_code.py --chat` opens an interactive
session: `/start` sends the complete task once, `/check` verifies observed results,
and `quit` exits. Ordinary input is sent one line at a time. Do not paste a
multiline task as if it were one message. Use `--verbose` for full tool output.

These are focused validation scenarios, not a general coding-agent benchmark.
They can report an incomplete scenario when a live model skips a required step.

## Web fetch and search

```sh
python examples/roomkit_web.py
python examples/roomkit_search.py
python examples/roomkit_search.py --service fixture-openai
```

Default runs use local HTTP fixtures and actual native requests. `--live` uses
your configured agent model while keeping local fixtures unless a real search
service is selected:

| Command suffix on `roomkit_search.py` | Search configuration |
| --- | --- |
| `--live --service exa` | Hosted Exa search; optional `EXA_API_KEY` |
| `--live --service ollama` | Hosted Ollama search; `OLLAMA_API_KEY` |
| `--live --service openai` | Responses web search; `OPENAI_SEARCH_API_KEY` and `OPENAI_SEARCH_MODEL` |

The example explicitly reads these variables. The library never discovers
credentials from the environment. Search credentials/model are separate from
the principal's `OPENAI_BASE_URL`, `OPENAI_MODEL` and `OPENAI_API_KEY`.
The search scenario requires fetching a URL actually returned by search, keeping
its query parameters. See the [web guide](../guide/web.md).

## Agent lifecycle

Start with `roomkit_delegate.py`. Use `roomkit_agents.py` to exercise more of the
[agent backend](../guide/agents.md):

```sh
python examples/roomkit_agents.py
python examples/roomkit_agents.py --dynamic
python examples/roomkit_agents.py --dynamic --parallel
python examples/roomkit_agents.py --live --dynamic --parallel
python examples/roomkit_agents.py --chat --hold --dynamic
```

Static profiles and dynamic host resolution use the same tools. The parallel
scenario checks two instances, targeted steering, independent cancellation,
retained follow-up history and principal report delivery. Workers only read files.

In interactive mode, `/start researcher` and `/start reviewer` launch through
the host. `/list`, `/cancel [task_id]`, `/followup [agent_id] [task]`, and
`/send [agent_id] [message]` inspect or control actual instances. Ordinary messages
go to the principal, which can call the same tools. Prefer explicit IDs when
several instances exist.

`--hold` is a demonstration pause, not a stalled worker. `/release [agent_id]`
releases it; `/pause [agent_id]` arms a later read. Without an ID, those commands
affect all existing instances. `/check` verifies evidence and linked replies.
`/diagnostics` shows bounded public RoomKit response/error summaries without
replaying calls. A completed model turn does not by itself prove a useful answer;
empty/truncated outputs and missing completion events require separate inspection.
`quit` closes owned tasks and resources.

`roomkit_background.py` demonstrates RoomKit's built-in background tools, rather
than the portable agent facade. It supports scripted, `--live`, and
`--chat --hold` modes. Its worker selects `code_outline`, `grep` and `read` from
this package; RoomKit supplies the orchestration and relays results to the
principal. Use `roomkit_agents.py` for profiles, messages and same-agent follow-ups.

## Rust examples

Run `cargo run --example NAME`. All argument-free runs use temporary fixtures
and verify their outputs without RoomKit, a model or external credentials.

| Name | Capability |
| --- | --- |
| `read_file` | Paginated text reading; optionally accepts a local path |
| `discover_files` | List, glob and grep |
| `edit_orders` | Revision-aware creation and edits using generic sample data |
| `patch_hours` | Comparison, preview and structured patch application |
| `shell_command` | Bounded command output and status |
| `process_session` | Persistent process I/O and cleanup |
| `read_output` | Archived output paging |
| `fetch_page` | Local HTTP redirect and Markdown conversion |
| `search_web` | Local search fixture; `--live` explicitly uses Exa |
| `outline_code` | Locate a method and read its source range |
| `view_image` | Prepare a PNG payload with bounded metadata |

[Documentation index](../guide/README.md) · [Project overview](../README.md)

## Optional local OCR

After installing the separate RustO backend and provisioning local PP-OCRv6
models, run `python examples/roomkit_ocr.py --models ./models --image
tests/fixtures/ocr/page.png` (on one line). This exercises real OCR through a
scripted RoomKit conversation. Add `--live` for the configured vLLM.
See [OCR](../guide/ocr.md); no model downloads occur in the example.
