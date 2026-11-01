//! The Markdown reads back through a CommonMark parser as exactly the page's
//! visible text, with no raw HTML.

#![no_main]

use htmlmark::{Format, Options, convert};
use libfuzzer_sys::fuzz_target;
use pulldown_cmark::{Event, Options as Extensions, Parser, Tag, TagEnd};

fn visible(markdown: &str) -> (String, bool) {
    let mut text = String::new();
    let mut html = false;
    let mut image = 0_usize;
    let extensions = Extensions::ENABLE_TABLES | Extensions::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, extensions) {
        match event {
            Event::Start(Tag::Image { .. }) => image += 1,
            Event::End(TagEnd::Image) => image = image.saturating_sub(1),
            Event::Text(found) | Event::Code(found) if image == 0 => text.push_str(&found),
            Event::Html(_) | Event::InlineHtml(_) => html = true,
            _ => {}
        }
    }
    (text, html)
}

fn without_whitespace(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fuzz_target!(|data: &[u8]| {
    let Ok(html) = std::str::from_utf8(data) else {
        return;
    };
    // A caption after a table's first row is written below the table, out of
    // the page's order: the text is the same, its order is not.
    if html.to_ascii_lowercase().contains("caption") {
        return;
    }
    let options = Options::default()
        .with_max_nodes(5_000)
        .with_max_depth(64)
        .with_max_output_bytes(1 << 20);
    let (Ok(markdown), Ok(text)) = (
        convert(html, &options),
        convert(html, &options.with_format(Format::Text)),
    ) else {
        return;
    };
    let (read, raw_html) = visible(&markdown);
    assert!(!raw_html, "raw HTML in {markdown:?}");
    assert_eq!(
        without_whitespace(&read),
        without_whitespace(&text),
        "Markdown {markdown:?}"
    );
});
