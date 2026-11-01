//! The Markdown reads back, through a CommonMark parser, as the same visible
//! text as the HTML it came from, with no raw HTML and only the page's links.
//!
//! Documents are generated from a fixed seed: nested blocks, lists, quotes,
//! tables, code and emphasis around text full of Markdown syntax characters.

use pulldown_cmark::{Event, Options as Extensions, Parser, Tag, TagEnd};

/// A small deterministic generator (xorshift64).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound).unwrap_or(u64::MAX);
        usize::try_from(self.next() % bound).unwrap_or(0)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        let index = self.below(items.len());
        items.get(index).copied().unwrap_or_default()
    }
}

/// Text fragments, as they appear on the page (HTML-escaped when written).
const PIECES: &[&str] = &[
    "word",
    "Ünï",
    "x",
    "a_b",
    "_x_",
    "*",
    "**",
    "`",
    "``",
    "[",
    "]",
    "(",
    ")",
    "!",
    "#",
    "# ",
    "1. ",
    "2) ",
    "- ",
    "+ ",
    "> ",
    "=",
    "---",
    "|",
    "~",
    "~~",
    "\\",
    "<",
    ">",
    "&",
    "&amp;",
    "&copy;",
    ":",
    "\"",
    "'",
    ".",
    ",",
    "12.50",
    "  ",
    "\n",
    "\t",
    "é",
    "Note:",
    "(x)",
    "#x",
    "foo #",
    "***",
    "___",
    "-x",
    "1.5",
    "[a]: /b",
    "<div>",
    "\u{a0}",
    "1",
    "2",
    ".",
    ")",
    "\u{301}",
    "\u{24b6}",
    "\u{200d}",
    "\u{b}",
    "\u{ab}",
    "\u{2019}",
    ":--",
    "-:",
    "\u{2003}x",
];

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn text(random: &mut Random) -> String {
    let pieces = 1 + random.below(4);
    (0..pieces)
        .map(|_| escape_html(random.pick(PIECES)))
        .collect()
}

const WRAPPERS: &[&str] = &[
    "p",
    "div",
    "b",
    "i",
    "strong",
    "em",
    "del",
    "code",
    "span",
    "h1",
    "h3",
    "blockquote",
    "a",
    "pre",
    "li-list",
    "ol-list",
    "table",
    "br",
    "hr",
    "img",
    "sup",
];

/// One random node, `depth` levels deep at most.
fn node(random: &mut Random, depth: usize) -> String {
    if depth == 0 || random.below(10) < 4 {
        return text(random);
    }
    let children = |random: &mut Random| -> String {
        (0..=random.below(3))
            .map(|_| node(random, depth - 1))
            .collect()
    };
    match random.pick(WRAPPERS) {
        "a" => {
            let target = random.below(5);
            format!(
                "<a href=\"https://example.com/{target}\">{}</a>",
                children(random)
            )
        }
        "img" => format!(
            "<img src=\"https://example.com/i{}.png\" alt=\"{}\">",
            random.below(5),
            text(random)
        ),
        "br" => "<br>".to_owned(),
        "hr" => "<hr>".to_owned(),
        "pre" => format!("<pre>{}\n{}</pre>", text(random), text(random)),
        "li-list" | "ol-list" => list(random, depth),
        "table" => table(random, depth),
        tag => element(tag, &children(random)),
    }
}

/// `<tag>` around `content`.
fn element(tag: &str, content: &str) -> String {
    format!("<{tag}>{content}</{tag}>")
}

fn list(random: &mut Random, depth: usize) -> String {
    let tag = if random.below(2) == 0 { "ul" } else { "ol" };
    let mut items = String::new();
    for _ in 0..=random.below(3) {
        items.push_str(&element("li", &node(random, depth - 1)));
    }
    element(tag, &items)
}

fn table(random: &mut Random, depth: usize) -> String {
    let columns = 1 + random.below(3);
    let mut header = String::new();
    for _ in 0..columns {
        header.push_str(&element("th", &text(random)));
    }
    // A caption first, where HTML puts it: one after a row is written below
    // the table, out of the page's order.
    let mut rows = if random.below(4) == 0 {
        element("caption", &text(random))
    } else {
        String::new()
    };
    rows.push_str(&element("tr", &header));
    for _ in 0..=random.below(3) {
        let mut cells = String::new();
        for _ in 0..=random.below(columns + 1) {
            cells.push_str(&element("td", &node(random, depth - 1)));
        }
        rows.push_str(&element("tr", &cells));
    }
    element("table", &rows)
}

/// What a CommonMark reader sees: visible characters, link destinations and
/// whether any raw HTML got through.
struct Reading {
    visible: String,
    links: Vec<String>,
    html: bool,
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only text, links and raw HTML matter here"
)]
fn read(markdown: &str) -> Reading {
    let mut reading = Reading {
        visible: String::new(),
        links: Vec::new(),
        html: false,
    };
    let mut in_image = 0;
    let extensions = Extensions::ENABLE_TABLES | Extensions::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, extensions) {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                in_image += 1;
                reading.links.push(dest_url.to_string());
            }
            Event::End(TagEnd::Image) => in_image -= 1,
            Event::Start(Tag::Link { dest_url, .. }) => reading.links.push(dest_url.to_string()),
            Event::Text(text) | Event::Code(text) if in_image == 0 => {
                reading.visible.push_str(&text);
            }
            Event::Html(_) | Event::InlineHtml(_) => reading.html = true,
            // Structure and breaks carry no page text.
            _ => {}
        }
    }
    reading
}

fn without_whitespace(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn markdown_reads_back_as_the_same_text_and_links() {
    // HTMLMARK_DOCUMENTS and HTMLMARK_SEED explore further than the default run.
    let documents =
        std::env::var("HTMLMARK_DOCUMENTS").map_or(4_000, |value| value.parse().unwrap());
    let seed = std::env::var("HTMLMARK_SEED")
        .map_or(0x9e37_79b9_7f4a_7c15, |value| value.parse().unwrap());
    let mut random = Random(seed);
    let mut failures = Vec::new();
    for _ in 0..documents {
        let html: String = (0..=random.below(4))
            .map(|_| node(&mut random, 4))
            .collect();
        let markdown = htmlmark::convert(&html, &htmlmark::Options::default()).unwrap();
        let text_options = htmlmark::Options::default().with_format(htmlmark::Format::Text);
        let expected = without_whitespace(&htmlmark::convert(&html, &text_options).unwrap());
        let reading = read(&markdown);
        let visible = without_whitespace(&reading.visible);
        let foreign_link = reading
            .links
            .iter()
            .find(|link| !link.starts_with("https://example.com/"));
        if visible != expected || reading.html || foreign_link.is_some() {
            failures.push(format!(
                "HTML:     {html:?}\nMarkdown: {markdown:?}\nexpected: {expected}\nread:     {visible}\nraw HTML: {}, foreign link: {foreign_link:?}",
                reading.html
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {documents} documents differ; first ones:\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}
