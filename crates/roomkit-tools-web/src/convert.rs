use crate::markdown_bound::{Element, Size};
use crate::{FetchFormat, parse};
use html5ever::Attribute;
use markup5ever_rcdom::{Handle, NodeData};
use reqwest::Url;
use roomkit_tools_core::Error;
use std::cell::RefCell;
use std::rc::Rc;
use tokio_util::sync::CancellationToken;

const SKIP: &[&str] = &[
    "head", "script", "style", "noscript", "template", "iframe", "object", "embed",
];

pub(crate) fn convert(
    source: String,
    html: bool,
    format: FetchFormat,
    base: &Url,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<String, Error> {
    if !html || format == FetchFormat::Html {
        return Ok(source);
    }
    let tree = parse::parse(&source, cancel)?;
    let mut walk = Walk {
        base,
        max_bytes,
        cancel,
        format,
        nodes: 0,
        link_bytes: 0,
        text: String::new(),
    };
    let markdown = walk.run(&tree)?;
    if format == FetchFormat::Text {
        return Ok(walk.text);
    }
    // Refuse before htmd allocates output that could exceed the limit.
    if markdown.bytes > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(too_long());
    }
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(SKIP.to_vec())
        .build();
    Ok(converter.tree_to_markdown(&tree))
}

fn too_long() -> Error {
    Error::LimitExceeded("converted text exceeds host byte limit".into())
}

/// One pass over the parsed DOM in document order: node and depth limits,
/// skipped subtrees, link expansion, plain text and the Markdown size bound.
struct Walk<'a> {
    base: &'a Url,
    max_bytes: usize,
    cancel: &'a CancellationToken,
    format: FetchFormat,
    nodes: usize,
    link_bytes: usize,
    text: String,
}

/// An element being walked and the index of its next child.
struct Frame {
    node: Handle,
    depth: usize,
    next: usize,
    markdown: Element,
}

impl Walk<'_> {
    /// Visit every node and return the bound of the Markdown htmd will write.
    fn run(&mut self, root: &Handle) -> Result<Size, Error> {
        self.visit(root, 0)?;
        let mut frames = vec![Frame {
            node: Rc::clone(root),
            depth: 0,
            next: 0,
            markdown: Element::document(),
        }];
        while let Some(frame) = frames.last_mut() {
            let Some(child) = frame.node.children.borrow().get(frame.next).cloned() else {
                if let Some(bound) = close_innermost(&mut frames) {
                    return Ok(bound);
                }
                continue;
            };
            frame.next += 1;
            let depth = frame.depth + 1;
            self.visit(&child, depth)?;
            match &child.data {
                NodeData::Element { name, attrs, .. } => {
                    let markdown = frame.markdown.open(
                        name.local.as_ref(),
                        &attrs.borrow(),
                        &child.children.borrow(),
                    );
                    frames.push(Frame {
                        node: child,
                        depth,
                        next: 0,
                        markdown,
                    });
                }
                NodeData::Text { contents } => frame.markdown.add_text(&contents.borrow()),
                NodeData::Document
                | NodeData::Doctype { .. }
                | NodeData::Comment { .. }
                | NodeData::ProcessingInstruction { .. } => {}
            }
        }
        Ok(Size::default())
    }

    fn visit(&mut self, node: &Handle, depth: usize) -> Result<(), Error> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.nodes += 1;
        if self.nodes > parse::MAX_NODES || depth > parse::MAX_DEPTH {
            return Err(parse::too_large());
        }
        match &node.data {
            NodeData::Element { name, attrs, .. } => {
                if SKIP.contains(&name.local.as_ref()) || is_hidden(&attrs.borrow()) {
                    node.children.borrow_mut().clear();
                    return Ok(());
                }
                self.expand_links(attrs)
            }
            NodeData::Text { contents } if self.format == FetchFormat::Text => {
                self.add_words(&contents.borrow())
            }
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::Text { .. }
            | NodeData::Comment { .. }
            | NodeData::ProcessingInstruction { .. } => Ok(()),
        }
    }

    /// Resolve `href`/`src` against the final URL and drop other schemes.
    fn expand_links(&mut self, attrs: &RefCell<Vec<Attribute>>) -> Result<(), Error> {
        attrs.borrow_mut().retain_mut(|a| {
            if !matches!(a.name.local.as_ref(), "href" | "src") {
                return true;
            }
            if let Ok(url) = self.base.join(&a.value)
                && matches!(url.scheme(), "http" | "https" | "mailto")
            {
                self.link_bytes = self.link_bytes.saturating_add(url.as_str().len());
                a.value = url.as_str().into();
                return true;
            }
            false
        });
        if self.link_bytes > self.max_bytes {
            return Err(Error::LimitExceeded(
                "expanded HTML links exceed host byte limit".into(),
            ));
        }
        Ok(())
    }

    fn add_words(&mut self, text: &str) -> Result<(), Error> {
        for word in text.split_whitespace() {
            if !self.text.is_empty() {
                self.text.push(' ');
            }
            self.text.push_str(word);
            if self.text.len() > self.max_bytes {
                return Err(too_long());
            }
        }
        Ok(())
    }
}

/// Close the innermost element into its parent; once the document itself
/// closes, return the bound of the whole Markdown.
fn close_innermost(frames: &mut Vec<Frame>) -> Option<Size> {
    let done = frames.pop()?;
    let bound = done
        .markdown
        .close(frames.last_mut().map(|parent| &mut parent.markdown));
    frames.is_empty().then_some(bound)
}

fn is_hidden(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.name.local.as_ref() == "hidden"
            || (a.name.local.as_ref() == "aria-hidden" && a.value.as_ref() == "true")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn markdown(html: &str, max_bytes: usize) -> Result<String, Error> {
        let base = Url::parse("https://example.com/docs/").unwrap();
        convert(
            html.to_owned(),
            true,
            FetchFormat::Markdown,
            &base,
            max_bytes,
            &CancellationToken::new(),
        )
    }

    fn outcome(result: &Result<String, Error>) -> String {
        result.as_ref().map_or_else(
            |error| format!("{error:?}"),
            |markdown| format!("converted {} bytes", markdown.len()),
        )
    }

    #[test]
    fn amplifying_markdown_is_refused_before_conversion() {
        // One wide header cell pads every row; nested quotes prefix every line.
        let table = format!(
            "<table><tr><th>{}</th></tr>{}</table>",
            "w".repeat(2_000),
            "<tr><td>x</td></tr>".repeat(2_000)
        );
        let quotes = format!(
            "{}<pre>{}</pre>",
            "<blockquote>".repeat(100),
            "x\n".repeat(2_000)
        );
        // htmd writes a math span's text verbatim, keeping spaces and newlines.
        let math_table = format!(
            "<table><tr><th>h</th></tr><tr><td>{}</td></tr>{}</table>",
            math_span("inline", &format!("a{}b", " ".repeat(5_000))),
            "<tr><td></td></tr>".repeat(200)
        );
        let math_quotes = format!(
            "{}{}",
            "<blockquote>".repeat(100),
            math_span("display", &"x\n".repeat(2_000))
        );
        for (name, html) in [
            ("table", table),
            ("quotes", quotes),
            ("math table", math_table),
            ("math quotes", math_quotes),
        ] {
            let result = markdown(&html, 256 * 1024);
            assert!(
                matches!(result, Err(Error::LimitExceeded(_))),
                "{name} ({} input bytes): {}",
                html.len(),
                outcome(&result)
            );
        }
    }

    /// A span htmd renders as math, with `text` as its only child.
    fn math_span(mode: &str, text: &str) -> String {
        format!("<span class=\"math math-{mode}\">{text}</span>")
    }

    /// The bound and the actual Markdown length for one document.
    fn bound_and_actual(html: &str) -> (u64, usize) {
        let base = Url::parse("https://example.com/docs/").unwrap();
        let cancel = CancellationToken::new();
        let tree = parse::parse(html, &cancel).unwrap();
        let mut walk = Walk {
            base: &base,
            max_bytes: usize::MAX,
            cancel: &cancel,
            format: FetchFormat::Markdown,
            nodes: 0,
            link_bytes: 0,
            text: String::new(),
        };
        let bound = walk.run(&tree).unwrap().bytes;
        let converter = htmd::HtmlToMarkdown::builder()
            .skip_tags(SKIP.to_vec())
            .build();
        (bound, converter.tree_to_markdown(&tree).len())
    }

    fn corpus() -> Vec<String> {
        let mut documents: Vec<String> = [
            "<html><head><title>Docs</title><style>HIDDEN</style></head><body><h1>Shipping API</h1><p>Free shipping starts at <strong>50 units</strong>.</p><p><a href=\"/guide\">Guide</a></p><pre><code class=\"language-python\">shipping_fee(50) == 0</code></pre><script>SECRET()</script><div hidden>HIDDEN</div></body></html>",
            "<p>*_[]`\\<a> 1. x # h - y + z > q === ~~~ |</p><p>1. item</p><p>- dash</p>",
            "<p>  a \n\n  b  </p><div>\t\tc\r\n d</div><p>&lt;div&gt; &amp; &nbsp; &#124;</p>",
            "<ol start=\"2147483646\"><li>a<ol><li>b<ul><li>c<br>d</li></ul></li></ol></li><li>e</li></ol>",
            "<blockquote><blockquote><pre><code class=\"language-rust\">fn x() {\n  ```\n}\n</code></pre></blockquote><p>t</p></blockquote>",
            "<p><code>`a`` b ```c</code> and <code>`</code> and <code>x|y</code></p>",
            "<a href=\"/x (1)\" title='say \"hi\"\nline2'>link text</a> <a href=\"https://e.com/a|b\">p|q</a><a>no href</a>",
            "<img src=\"/i.png\" alt=\"multi\nline &quot;alt&quot;\" title=\"t|t\"><img alt=\"no source\">",
            "<h1>T</h1><h6>x</h6><hr><p>a<br>b<br><br>c</p><span class=\"math math-inline\">x^2</span><span class=\"math math-display\">y</span>",
            "<table><caption>Cap\nline</caption><thead><tr><th>h|1</th><th>é</th></tr></thead><tbody><tr><td>éééé</td></tr><tr><td>a</td><td>b</td><td>c</td><td>d</td></tr></tbody><tfoot><tr><td>f</td></tr></tfoot></table>",
            "<table><tr><th>h</th></tr><tr><td><table><tr><th>in</th></tr><tr><td>x</td></tr></table><ul><li>a</li><li>b</li></ul><blockquote>q<br>r</blockquote><pre>p\nq</pre></td></tr></table>",
            "<ul><li><blockquote><table><tr><th>a</th><th>b</th></tr><tr><td>1</td><td>2</td></tr></table></blockquote></li></ul>",
            "<table><tr><td>a</td><td>b</td></tr></table><table><tr><th>名前</th></tr><tr><td>東京都</td></tr><tr><td>abcdefgh</td></tr></table>",
            "<pre>~~~\n```x</pre><pre>`tick</pre><pre><b>bold</b>\n<i>it</i></pre>",
            "<div hidden><a href=\"javascript:x\">y</a></div><a hidden href=\"javascript:z\">w</a><a aria-hidden=\"true\">v</a>",
            "<dl><dt>t</dt><dd>d</dd></dl><details><summary>s</summary>body</details><form><textarea>a\nb</textarea><select><option>o</option></select></form>",
            "<p><b>bold</b><strong>strong</strong><i>i</i><em>em</em><b> spaced </b><b></b><u>u</u><small>s</small></p>",
            "<ul><li><p>para</p><pre>a\nb\nc</pre><ol start=\"98\"><li>x</li><li>y</li><li>z</li></ol></li></ul>",
            "<table><tr><th>`code` |</th><th><a href=\"/l\" title=\"t\">link</a></th></tr><tr><td><img src=\"/i\" alt=\"a|b\"></td><td><code>``</code></td></tr></table>",
            "<svg><a href=\"/s\"><text>t</text></a></svg><math><mi>x</mi></math><unknown-tag>u\nv</unknown-tag>",
        ]
        .map(str::to_owned)
        .to_vec();
        documents.push(format!(
            "<table><tr><th>{}</th></tr>{}</table>",
            "w".repeat(50),
            "<tr><td>x</td></tr>".repeat(50)
        ));
        documents.push(format!(
            "{}<pre>{}</pre>",
            "<blockquote>".repeat(20),
            "x\n".repeat(20)
        ));
        documents.push(format!(
            "<div>{}</div>{}",
            "<a href=x title=yy><b i=1><i i=2>",
            "<p>x</p>".repeat(30)
        ));
        documents.push(format!(
            "<table><tr><th>h</th></tr><tr>{}</tr>{}</table>",
            "<td>x</td>".repeat(40),
            "<tr><td>y</td></tr>".repeat(40)
        ));
        documents.push(article(40));
        documents.push(navigation(30));
        documents.push(discussion(20));
        documents.extend(math_documents());
        documents.extend(layout_tables());
        documents
    }

    /// Math spans, written verbatim by htmd, in every context that amplifies
    /// text: padded cells, prefixed lines, code fences.
    fn math_documents() -> Vec<String> {
        let spaced = format!("a{}b", " ".repeat(500));
        let lines = "x\n".repeat(20);
        vec![
            format!(
                "<table><tr><th>h</th></tr><tr><td>{}</td></tr>{}</table>",
                math_span("inline", &spaced),
                "<tr><td></td></tr>".repeat(20)
            ),
            format!(
                "{}{}",
                "<blockquote>".repeat(20),
                math_span("display", &lines)
            ),
            format!(
                "<ul><li>{}<ol><li>{}</li></ol></li></ul>",
                math_span("display", &lines),
                math_span("inline", "\r\n\t |`` ~ é")
            ),
            format!(
                "<table><tr><th>{}</th></tr><tr><td>{}{}</td></tr></table>",
                math_span("inline", "| a | b |"),
                math_span("display", "`x`\n``y``"),
                math_span("display", " ")
            ),
            format!(
                "<pre><code>{}</code></pre><p>{}</p><p>{}{}</p>",
                math_span("display", "```\n~~~"),
                math_span("inline", "*_[]\\<b>"),
                math_span("inline", "a  b"),
                math_span("inline", "c\n\nd")
            ),
            // Not math for htmd: a second attribute, another class, element
            // children, or no text at all.
            format!(
                "<p><span class=\"math math-inline\" id=\"m\">{spaced}</span><span class=\"math\">{spaced}</span><span class=\"math math-display\"><b>{spaced}</b></span><span class=\"math math-inline\"></span></p>"
            ),
        ]
    }

    /// Tables without header cells, or nested in a cell, which htmd writes as
    /// plain content, next to header tables it pads.
    fn layout_tables() -> Vec<String> {
        vec![
            "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>".to_owned(),
            "<table><tr><th>h</th></tr><tr><td><table><tr><th>inner</th></tr><tr><td>x</td></tr></table></td></tr></table>".to_owned(),
            "<table><tr><td><table><tr><td><table><tr><th>deep</th></tr></table></td></tr></table></td></tr></table>".to_owned(),
            "<table><thead><tr><td>h</td></tr></thead><tr><td>x</td></tr></table>".to_owned(),
            "<table><tr><td><div><span>th</span></div></td></tr><caption>c</caption></table>".to_owned(),
            format!(
                "<table><tr><td>{}</td></tr></table><table><tr><th>{}</th></tr><tr><td>x</td></tr></table>",
                "w".repeat(200),
                "<table><tr><td>y</td></tr></table>"
            ),
        ]
    }

    /// A discussion page laid out with tables: a page table holding a comment
    /// table, one single-row table per comment, and no header cells.
    fn discussion(comments: usize) -> String {
        let text = "This matches what we saw in production: the cache was warm, \
            but the first request after a deploy still paid for the schema load. \
            Moving it to startup fixed the latency spike for us. ";
        let mut html = String::from(
            "<html><body><center><table id=\"page\" width=\"85%\"><tr><td>\
             <table class=\"comments\" border=\"0\">\n",
        );
        for comment in 0..comments {
            writeln!(
                html,
                "<tr class=\"comment\" id=\"c{comment}\"><td><table border=\"0\"><tr>\
                 <td class=\"indent\"><img src=\"/s.gif\" height=\"1\" width=\"{indent}\"></td>\
                 <td class=\"votes\"><a href=\"/vote?id={comment}&amp;how=up\"><div class=\"arrow\" title=\"upvote\"></div></a></td>\
                 <td class=\"body\"><div class=\"head\"><a href=\"/user?id=reader{comment}\" class=\"user\">reader{comment}</a> \
                 <span class=\"age\"><a href=\"/item?id={comment}\">{hours} hours ago</a></span></div>\
                 <div class=\"text\"><p>{text}{text}{text}</p><p>{text}{text}</p><p>{text}{text}{text}</p></div>\
                 <div class=\"reply\"><a href=\"/reply?id={comment}&amp;goto=item\">reply</a></div></td></tr></table></td></tr>",
                indent = (comment % 5) * 40,
                hours = comment % 24
            )
            .unwrap();
        }
        html.push_str("</table></td></tr></table></center></body></html>");
        html
    }

    /// A documentation page: sections with prose, links, inline code, a code
    /// block, a list and a small table, indented as a typical page source.
    fn article(sections: usize) -> String {
        let mut html = String::from("<html><body><main>\n");
        for section in 0..sections {
            write!(
                html,
                "  <h2 id=\"s{section}\">Section {section}</h2>\n  <p>\n    The <a href=\"/api/v1/items#{section}\" title=\"Item reference\">items endpoint</a> returns\n    a page of results; pass <code>cursor</code> to continue. See also\n    <a href=\"https://example.org/guides/pagination?from=docs\">the guide</a>.\n  </p>\n  <pre><code class=\"language-python\">for item in client.items(cursor=None):\n    print(item.name, item.price * 2)\n</code></pre>\n  <ul>\n    <li>First <em>point</em></li>\n    <li>Second point with <strong>emphasis</strong></li>\n  </ul>\n  <table>\n    <tr><th>Field</th><th>Type</th></tr>\n    <tr><td>name</td><td>string</td></tr>\n    <tr><td>price_total</td><td>decimal</td></tr>\n  </table>\n"
            )
            .unwrap();
        }
        html.push_str("</main></body></html>");
        html
    }

    /// A site menu: nested lists of links, three levels deep.
    fn navigation(entries: usize) -> String {
        let mut html = String::from("<nav><ul>\n");
        for top in 0..entries {
            write!(
                html,
                "  <li><a href=\"/section/{top}/\">Section {top}</a>\n    <ul>\n"
            )
            .unwrap();
            for sub in 0..5 {
                write!(
                    html,
                    "      <li><a href=\"/section/{top}/page-{sub}\">Page {sub}</a>\n        <ul><li><a href=\"/section/{top}/page-{sub}#top\">Top</a></li></ul>\n      </li>\n"
                )
                .unwrap();
            }
            html.push_str("    </ul>\n  </li>\n");
        }
        html.push_str("</ul></nav>");
        html
    }

    #[test]
    fn markdown_bound_covers_the_converted_output() {
        for html in corpus() {
            let (bound, actual) = bound_and_actual(&html);
            assert!(
                bound >= u64::try_from(actual).unwrap(),
                "bound {bound} < actual {actual} for {html}"
            );
        }
    }

    /// Table rows of an id, a name and, optionally, a price.
    fn item_rows(count: usize, price: bool) -> String {
        let mut rows = String::new();
        for row in 0..count {
            write!(rows, "<tr><td>{row}</td><td>Item name {row}</td>").unwrap();
            if price {
                rows.push_str("<td>12.50</td>");
            }
            rows.push_str("</tr>");
        }
        rows
    }

    #[test]
    fn markdown_bound_stays_close_on_ordinary_pages() {
        let table = format!(
            "<table><tr><th>Id</th><th>Name</th></tr>{}</table>",
            item_rows(1_000, false)
        );
        for (name, html) in [
            ("article", article(200)),
            ("navigation", navigation(100)),
            ("table", table),
            ("discussion", discussion(300)),
        ] {
            let (bound, actual) = bound_and_actual(&html);
            assert!(
                bound <= 2 * u64::try_from(actual).unwrap(),
                "{name}: bound {bound} for {actual} bytes"
            );
        }
    }

    #[test]
    fn large_tables_with_ordinary_cells_still_convert() {
        let html = format!(
            "<table><tr><th>Id</th><th>Name</th><th>Price</th></tr>{}</table>",
            item_rows(1_000, true)
        );
        let markdown = markdown(&html, 5 * 1024 * 1024).unwrap();
        assert!(
            markdown.contains("| 999 | Item name 999 | 12.50 |"),
            "{markdown}"
        );
    }
}
