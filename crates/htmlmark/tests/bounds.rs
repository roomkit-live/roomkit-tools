//! Limits a hostile page cannot get around.

use std::cell::Cell;
use std::fmt::Write as _;

use htmlmark::{Error, Format, Options, convert};

fn documents() -> Vec<String> {
    vec![
        "<h1>Title</h1><p>Some <b>bold</b> and <a href=\"/x\">a link</a>.</p>".to_owned(),
        "<ul><li>one<ul><li>two</li></ul></li></ul><blockquote><pre>a\nb</pre></blockquote>"
            .to_owned(),
        "<table><tr><th>h</th><th>i</th></tr><tr><td>1</td><td>2</td></tr></table>".to_owned(),
        format!("<p>{}</p>", "word ".repeat(500)),
    ]
}

#[test]
fn the_output_limit_is_exact() {
    for html in documents() {
        for format in [Format::Markdown, Format::Text] {
            let full = convert(&html, &Options::default().with_format(format)).unwrap();
            let fits = Options::default()
                .with_format(format)
                .with_max_output_bytes(full.len());
            assert_eq!(convert(&html, &fits).unwrap(), full);
            let short = fits.with_max_output_bytes(full.len() - 1);
            assert_eq!(convert(&html, &short), Err(Error::OutputTooLong), "{html}");
        }
    }
}

#[test]
fn markdown_that_amplifies_its_page_is_refused_at_the_limit() {
    // Each enclosing quote prefixes every line of the code inside again:
    // about 40 KB of page would write about 800 KB of Markdown.
    let quotes = format!(
        "{}<pre>{}</pre>",
        "<blockquote>".repeat(100),
        "x\n".repeat(2_000)
    );
    let options = Options::default().with_max_output_bytes(256 * 1024);
    assert_eq!(convert(&quotes, &options), Err(Error::OutputTooLong));
    // Missing cells of a wide table are written as empty cells.
    let table = format!(
        "<table><tr><th>h</th></tr><tr>{}</tr>{}</table>",
        "<td>x</td>".repeat(2_000),
        "<tr><td>y</td></tr>".repeat(2_000)
    );
    assert_eq!(convert(&table, &options), Err(Error::OutputTooLong));
}

/// `width` formatting elements left open, then `runs` blocks of text: the
/// parser recreates every open formatting element inside each block.
#[expect(clippy::unwrap_used, reason = "writing to a String cannot fail")]
fn reopened_formatting(width: usize, runs: usize) -> String {
    let mut html = String::from("<div>");
    for index in 0..width {
        write!(html, "<b i={index}>").unwrap();
    }
    html.push_str("</div>");
    html.push_str(&"<div>x</div>".repeat(runs));
    html
}

#[test]
fn documents_beyond_the_node_or_depth_limit_are_refused() {
    let options = Options::default();
    assert_eq!(
        convert(&reopened_formatting(100, 1_100), &options),
        Err(Error::TooLarge)
    );
    assert_eq!(
        convert(&"<div>".repeat(200), &options),
        Err(Error::TooLarge)
    );
    let small = Options::default().with_max_nodes(50).with_max_depth(10);
    assert_eq!(
        convert(&"<p>x</p>".repeat(30), &small),
        Err(Error::TooLarge)
    );
    assert_eq!(convert(&"<div>".repeat(12), &small), Err(Error::TooLarge));
    assert!(convert(&"<div>".repeat(5), &small).is_ok());
}

#[test]
fn rendering_polls_the_interrupt_callback() {
    // Under 1 KiB of source, so one parser chunk, but about 600 nodes.
    let html = reopened_formatting(20, 30);
    assert!(html.len() < 1024);
    let calls = Cell::new(0);
    let stop_at = Cell::new(usize::MAX);
    let interrupt = || {
        calls.set(calls.get() + 1);
        calls.get() >= stop_at.get()
    };
    let options = Options::default().with_interrupt(&interrupt);
    convert(&html, &options).unwrap();
    let parse_calls = 2;
    assert!(calls.get() > parse_calls, "{} calls", calls.get());
    // Stopping at the first call after parsing interrupts the rendering.
    calls.set(0);
    stop_at.set(parse_calls + 1);
    assert_eq!(convert(&html, &options), Err(Error::Interrupted));
}

/// Malformed fragments: stray and unclosed tags, attributes, entities, raw
/// syntax characters and control bytes.
const SOUP: &[&str] = &[
    "<b>",
    "</b>",
    "<i>",
    "</i>",
    "<a href=x>",
    "</a>",
    "<a>",
    "<table>",
    "<tr>",
    "<td>",
    "<th>",
    "</table>",
    "<ul>",
    "<li>",
    "</ul>",
    "<ol start=-3>",
    "<pre>",
    "</pre>",
    "<code>",
    "<br>",
    "<hr>",
    "<h2>",
    "</h2>",
    "<blockquote>",
    "<p>",
    "</p>",
    "<svg><text>",
    "<math>",
    "<select>",
    "<option>",
    "<template>",
    "<html a=1>",
    "<body b=2>",
    "<!--",
    "-->",
    "<![CDATA[",
    "&amp",
    "&#0;",
    "&#x110000;",
    "\u{0}",
    "\u{1b}",
    "\u{a0}",
    "*",
    "_",
    "`",
    "[",
    "]",
    "#",
    "|",
    "~",
    "\\",
    "<",
    ">",
    "text",
    "\n",
    " ",
];

#[test]
fn malformed_pages_never_panic_and_stay_within_the_output_limit() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % 1_000_003).unwrap_or(0)
    };
    for _ in 0..2_000 {
        let pieces = 1 + next() % 120;
        let html: String = (0..pieces)
            .map(|_| SOUP.get(next() % SOUP.len()).copied().unwrap_or_default())
            .collect();
        for format in [Format::Markdown, Format::Text] {
            let limit = 1 + next() % 400;
            let options = Options::default()
                .with_format(format)
                .with_max_output_bytes(limit)
                .with_max_nodes(2_000)
                .with_max_depth(40);
            match convert(&html, &options) {
                Ok(output) => assert!(output.len() <= limit, "{html:?}"),
                Err(Error::OutputTooLong | Error::TooLarge) => {}
                Err(error) => panic!("{error:?} for {html:?}"),
            }
        }
    }
}

#[test]
fn long_code_blocks_poll_the_interrupt_callback_while_written() {
    // Same length, same nodes: only the number of code lines differs.
    let lines = format!("<pre>{}x</pre>", "\n".repeat(100_000));
    let one_line = format!("<pre>{}x</pre>", "y".repeat(100_000));
    let count = |html: &str| {
        let calls = Cell::new(0);
        let interrupt = || {
            calls.set(calls.get() + 1);
            false
        };
        convert(html, &Options::default().with_interrupt(&interrupt)).unwrap();
        calls.get()
    };
    assert!(count(&lines) > count(&one_line));
}
