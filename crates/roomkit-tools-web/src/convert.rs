//! HTML to Markdown or text through `htmlmark`, with the tool's limits.
//!
//! Links and images resolve against the final URL; other schemes than HTTP(S)
//! and `mailto` are dropped. Expanded URLs are counted as they are resolved,
//! whether or not they end up in the output, and bounded like the output.

use std::cell::{Cell, RefCell};

use crate::FetchFormat;
use reqwest::Url;
use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

/// Most nodes a document may have.
const MAX_NODES: usize = 100_000;
/// Deepest nesting below the document node.
const MAX_DEPTH: usize = 128;

/// Why a conversion stopped early, beyond what `htmlmark` reports.
#[derive(Clone, Copy)]
enum Stop {
    Cancelled,
    Links,
}

pub(crate) fn convert(
    source: String,
    html: bool,
    format: FetchFormat,
    base: &Url,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<String, ToolError> {
    let format = match format {
        FetchFormat::Markdown if html => htmlmark::Format::Markdown,
        FetchFormat::Text if html => htmlmark::Format::Text,
        FetchFormat::Markdown | FetchFormat::Text | FetchFormat::Html => return Ok(source),
    };
    let link_bytes = Cell::new(0_usize);
    let stop = RefCell::new(None);
    let resolve = |value: &str| {
        let url = base
            .join(value)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https" | "mailto"))?;
        link_bytes.set(link_bytes.get().saturating_add(url.as_str().len()));
        if link_bytes.get() > max_bytes {
            stop.replace(Some(Stop::Links));
        }
        Some(String::from(url))
    };
    let interrupt = || {
        if cancel.is_cancelled() {
            stop.borrow_mut().get_or_insert(Stop::Cancelled);
        }
        stop.borrow().is_some()
    };
    let options = htmlmark::Options::default()
        .with_format(format)
        .with_max_nodes(MAX_NODES)
        .with_max_depth(MAX_DEPTH)
        .with_max_output_bytes(max_bytes)
        .with_rewrite_url(&resolve)
        .with_interrupt(&interrupt);
    let result = htmlmark::convert(&source, &options);
    // A short page can end before the next interrupt check.
    if matches!(*stop.borrow(), Some(Stop::Links)) {
        return Err(links_too_long());
    }
    result.map_err(|error| failure(error, *stop.borrow()))
}

fn links_too_long() -> ToolError {
    ToolError::LimitExceeded("expanded HTML links exceed host byte limit".into())
}

/// The tool error for a failed conversion; an error `htmlmark` adds later is
/// still a refused page.
fn failure(error: htmlmark::Error, stop: Option<Stop>) -> ToolError {
    match (error, stop) {
        (htmlmark::Error::OutputTooLong, _) => {
            ToolError::LimitExceeded("converted text exceeds host byte limit".into())
        }
        (_, Some(Stop::Links)) => links_too_long(),
        (htmlmark::Error::Interrupted, _) => ToolError::Cancelled,
        _ => ToolError::LimitExceeded("HTML exceeds 100000 nodes or 128 levels".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn convert_with(
        html: &str,
        format: FetchFormat,
        max_bytes: usize,
    ) -> Result<String, ToolError> {
        let base = Url::parse("https://example.com/docs/").unwrap();
        convert(
            html.to_owned(),
            true,
            format,
            &base,
            max_bytes,
            &CancellationToken::new(),
        )
    }

    fn markdown(html: &str, max_bytes: usize) -> Result<String, ToolError> {
        convert_with(html, FetchFormat::Markdown, max_bytes)
    }

    #[test]
    fn links_resolve_against_the_page_and_other_schemes_are_dropped() {
        let html = "<p><a href=\"../guide\">Guide</a> <a href=\"javascript:x()\">run</a> <img src=\"i.png\" alt=\"logo\"> <a href=\"mailto:a@example.com\">mail</a></p>";
        assert_eq!(
            markdown(html, 4096).unwrap(),
            "[Guide](https://example.com/guide) run ![logo](https://example.com/docs/i.png) [mail](mailto:a@example.com)"
        );
    }

    #[test]
    fn expanded_links_are_bounded_even_when_not_shown() {
        // Empty links write nothing, but each one resolves to a long URL.
        let html = "<a href=\"relative\"></a>".repeat(100);
        assert!(matches!(
            markdown(&html, 1024),
            Err(ToolError::LimitExceeded(message)) if message.contains("links")
        ));
    }

    #[test]
    fn limits_and_cancellation_map_to_tool_errors() {
        let quotes = format!(
            "{}<pre>{}</pre>",
            "<blockquote>".repeat(100),
            "x\n".repeat(2_000)
        );
        assert!(matches!(
            markdown(&quotes, 256 * 1024),
            Err(ToolError::LimitExceeded(message)) if message.contains("converted text")
        ));
        assert!(matches!(
            markdown(&"<div>".repeat(200), 4096),
            Err(ToolError::LimitExceeded(message)) if message.contains("nodes")
        ));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let base = Url::parse("https://example.com/").unwrap();
        let result = convert(
            "<p>x</p>".to_owned(),
            true,
            FetchFormat::Markdown,
            &base,
            4096,
            &cancel,
        );
        assert!(matches!(result, Err(ToolError::Cancelled)));
    }

    /// Table rows of an id, a name and a price.
    fn item_rows(count: usize) -> String {
        let mut rows = String::new();
        for row in 0..count {
            write!(
                rows,
                "<tr><td>{row}</td><td>Item name {row}</td><td>12.50</td></tr>"
            )
            .unwrap();
        }
        rows
    }

    #[test]
    fn large_tables_with_ordinary_cells_still_convert() {
        let html = format!(
            "<table><tr><th>Id</th><th>Name</th><th>Price</th></tr>{}</table>",
            item_rows(1_000)
        );
        let markdown = markdown(&html, 5 * 1024 * 1024).unwrap();
        assert!(
            markdown.contains("| 999 | Item name 999 | 12.50 |"),
            "{markdown}"
        );
    }

    #[test]
    fn text_keeps_the_visible_words() {
        let html = "<h1>Shipping API</h1><p>Free <b>shipping</b></p><script>x()</script>";
        assert_eq!(
            convert_with(html, FetchFormat::Text, 4096).unwrap(),
            "Shipping API Free shipping"
        );
    }
}
