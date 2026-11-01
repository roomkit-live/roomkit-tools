# htmlmark

Bounded, interruptible HTML to Markdown and plain-text conversion, built for
pages you do not trust.

`htmlmark` parses with [html5ever](https://crates.io/crates/html5ever), the
WHATWG-compliant parser of the Servo project, into its own compact tree, then
writes CommonMark (with GitHub-flavored pipe tables and strikethrough) or the
page's visible words. It does not use `markup5ever_rcdom`, html5ever's test
DOM, which its authors do not support and have not tested against hostile
input.

## Why another converter

A page from the web can be built to hurt its reader:

- unclosed formatting tags make the HTML parser recreate elements, so a short
  page can expand into hundreds of thousands of nodes;
- deep nesting exhausts the stack of a recursive walk;
- nested quotes and lists prefix every line again, so the Markdown can be many
  times larger than the page;
- text full of `*`, `_`, `[` or `#` turns into formatting, links or headings
  that were never there.

`htmlmark` handles each of these:

- the parse stops once the document holds more than `max_nodes` nodes or
  nests deeper than `max_depth`, counting recreated elements as they appear;
- rendering walks the tree without recursion;
- the output is refused as soon as it would exceed `max_output_bytes`: the
  limit is checked on every write, not estimated;
- an `interrupt` callback is polled between 1 KiB parser chunks, every few
  hundred rendered nodes and every few thousand lines of a code block, so a
  caller can enforce a deadline or cancellation;
- text is escaped wherever Markdown would read it as syntax.

## Usage

```rust
use htmlmark::{Error, Format, Options, convert};

fn page_to_markdown(html: &str, base: &str, deadline_passed: &dyn Fn() -> bool) -> Result<String, Error> {
    // Make every link absolute and drop the ones that are not web links.
    let resolve = |url: &str| {
        let absolute = if url.starts_with('/') { format!("{base}{url}") } else { url.to_owned() };
        absolute.starts_with("https://").then_some(absolute)
    };
    let options = Options::default()
        .with_format(Format::Markdown)
        .with_max_output_bytes(1024 * 1024)
        .with_rewrite_url(&resolve)
        .with_interrupt(deadline_passed);
    convert(html, &options)
}
```

Defaults: 100,000 nodes, depth 128, 5 MiB of output, URLs kept as written, no
interruption. A failed conversion returns `Error::TooLarge`,
`Error::OutputTooLong` or `Error::Interrupted`, never partial output.

## What it writes

| HTML | Markdown |
| --- | --- |
| `h1` to `h6` | `#` to `######` headings |
| `p`, `div`, `section` and other blocks | paragraphs separated by a blank line |
| `ul`, `ol` (with `start`), `li` | `- item`, `1. item`, nested by indentation |
| `blockquote` | `> ` on every line |
| `pre`, `pre > code.language-x` | fenced code block, `x` as its language |
| `code`, `kbd`, `samp` | code span |
| `b`, `strong` / `i`, `em` / `del`, `s` | `**bold**` / `*italic*` / `~~struck~~` |
| `a href`, `img src` | `[text](url "title")`, `![alt](url "title")` |
| `br`, `hr` | hard line break, `* * *` |
| table with `th` cells | pipe table, first row as header, cells not padded |
| other tables (layout) | their cells as paragraphs |

`head`, `script`, `style`, `noscript`, `template`, `iframe`, `object`, `embed`
and elements marked `hidden` or `aria-hidden="true"` are left out. SVG and
MathML show their text only.

Emphasis is written only where a CommonMark reader will see it as emphasis:
`<b>Note:</b>Text` gives `Note:Text` rather than `**Note:**Text`, which would
show its asterisks. The text is always kept; only the emphasis is lost.

## How it is tested

Besides tests for each element and each limit, a property test generates
thousands of random documents (nested blocks, lists, quotes, tables, code and
emphasis around text full of Markdown syntax characters), reads the Markdown
back with [pulldown-cmark](https://crates.io/crates/pulldown-cmark), and checks
that it shows exactly the page's visible text, no raw HTML and only the page's
links. `HTMLMARK_DOCUMENTS` and `HTMLMARK_SEED` run it further:

```sh
HTMLMARK_DOCUMENTS=50000 HTMLMARK_SEED=7 cargo test --release -p htmlmark --test commonmark
```

Two [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets in `fuzz/`
need a nightly toolchain. `convert` feeds arbitrary input with random limits
and checks that nothing panics and the output stays within its limit;
`read_back` checks the same read-back property as the test above, guided by
coverage instead of a generator:

```sh
cd crates/htmlmark
cargo +nightly fuzz run read_back -- -dict=fuzz/html.dict -max_len=4096
```

## Status

`htmlmark` is developed inside
[roomkit-tools](https://github.com/roomkit-live/roomkit-tools), where it
converts pages for the `web_fetch` tool, and is not published on crates.io
yet. Its API may change before a first release. Minimum supported Rust version:
1.94. License: MIT.
