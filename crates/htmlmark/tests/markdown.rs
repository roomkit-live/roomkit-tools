//! The Markdown written for each kind of element.

use htmlmark::{Options, convert};

#[expect(clippy::unwrap_used, reason = "every example converts")]
fn markdown(html: &str) -> String {
    convert(html, &Options::default()).unwrap()
}

#[test]
fn blocks_are_separated_by_blank_lines() {
    assert_eq!(
        markdown(
            "<h2>Head <b>x</b></h2><p>a<br>b</p><hr><div>one</div><div>two</div><span>inline</span> after"
        ),
        "## Head **x**\n\na  \nb\n\n* * *\n\none\n\ntwo\n\ninline after"
    );
}

#[test]
fn text_that_reads_as_syntax_is_escaped() {
    assert_eq!(
        markdown(
            "<p>*stars* _under_ snake_case [1] `tick` &lt;div&gt; AT&amp;T &amp;copy; C:\\dir ~x~ a|b</p>"
        ),
        "\\*stars\\* \\_under\\_ snake_case \\[1\\] \\`tick\\` \\<div> AT\\&T \\&copy; C:\\\\dir \\~x\\~ a|b"
    );
    // Only where a line starts can text begin a heading, quote, list or rule.
    assert_eq!(
        markdown(
            "<p># h</p><p>1. one</p><p>2) two</p><p>- dash</p><p>+ plus</p><p>&gt; q</p><p>===</p><p>12.50 -5 +x</p>"
        ),
        "\\# h\n\n1\\. one\n\n2\\) two\n\n\\- dash\n\n\\+ plus\n\n\\> q\n\n\\===\n\n12.50 -5 +x"
    );
    assert_eq!(markdown("<h3>1. Intro #</h3>"), "### 1. Intro \\#");
}

#[test]
fn emphasis_keeps_its_spaces_outside_and_is_dropped_where_it_would_not_read() {
    assert_eq!(
        markdown(
            "<p>a<b> spaced </b>c <b></b>d <i>it</i> <del>gone</del> <b>bold <i>both</i></b></p>"
        ),
        "a **spaced** c d *it* ~~gone~~ **bold *both***"
    );
    // CommonMark would show these markers as asterisks: the text stays.
    assert_eq!(markdown("<p><b>Note:</b>Text</p>"), "Note:Text");
    assert_eq!(markdown("<p>a<b>\"b\"</b></p>"), "a\"b\"");
    // The same emphasis twice in a row continues; nested, it adds nothing.
    assert_eq!(
        markdown("<p><b>a</b><b>b</b> <b><strong>c</strong></b></p>"),
        "**ab** **c**"
    );
}

#[test]
fn lists_nest_with_their_markers_and_numbers() {
    assert_eq!(
        markdown(
            "<ul><li>one<ul><li>two</li></ul></li><li>three</li></ul><ol start=\"9\"><li>nine</li><li>ten<ol><li>a</li></ol></li></ol><p>after</p>"
        ),
        "- one\n  - two\n- three\n\n9. nine\n10. ten\n\n    1. a\n\nafter"
    );
    assert_eq!(
        markdown(
            "<ul><li><p>para</p><pre>code</pre></li><li><blockquote>quoted</blockquote></li></ul>"
        ),
        "- para\n\n  ```\n  code\n  ```\n- > quoted"
    );
}

#[test]
fn quotes_prefix_every_line() {
    assert_eq!(
        markdown("<blockquote><p>q1</p><p>q2</p><blockquote>deep</blockquote></blockquote>"),
        "> q1\n>\n> q2\n>\n> > deep"
    );
}

#[test]
fn code_keeps_its_text_inside_long_enough_fences() {
    assert_eq!(
        markdown("<pre><code class=\"language-rust\">fn x() {\n    ```\n}\n</code></pre>"),
        "````rust\nfn x() {\n    ```\n}\n````"
    );
    // Adjacent code spans would join their fences: the second one is text,
    // the third code again.
    assert_eq!(
        markdown(
            "<p><code>a</code><code>b*</code><code>c</code> <b>x<code>d</code></b><code>e</code></p>"
        ),
        "`a`b\\*`c` **x`d`**`e`"
    );
    // An empty code element between them changes nothing; a space separates
    // their fences.
    assert_eq!(
        markdown("<p><code>a</code><code> </code><code>b</code></p>"),
        "`a` `b`"
    );
    assert_eq!(
        markdown("<p><code>a</code><code></code><code>b</code></p>"),
        "`a`b"
    );
    // Even once the same emphasis continues across the closer between them.
    assert_eq!(
        markdown("<p><b>x<code>a</code></b><b><code>b</code></b></p>"),
        "**x`a`b**"
    );
    assert_eq!(
        markdown("<p><code>a`b</code> and <code> spaced </code>x <code>*no* [escape]</code></p>"),
        "``a`b`` and `spaced` x `*no* [escape]`"
    );
}

#[test]
fn links_and_images_keep_destinations_and_titles() {
    assert_eq!(
        markdown(
            "<p><a href=\"https://e.com/x\" title='say \"hi\"'>link</a> <a href=\"/a b(c)\"><img src=\"/i.png\" alt=\"[x]\"></a> Hi!<a href=\"/z\">z</a></p>"
        ),
        "[link](https://e.com/x \"say \\\"hi\\\"\") [![\\[x\\]](/i.png)](/a%20b\\(c\\)) Hi\\![z](/z)"
    );
    // Empty links show nothing; images without a source are dropped.
    assert_eq!(
        markdown("<p>a<a href=\"/x\"></a><img alt=\"no source\">b</p>"),
        "ab"
    );
}

#[test]
fn urls_go_through_the_rewrite() {
    let rewrite =
        |url: &str| (!url.starts_with("javascript:")).then(|| format!("https://e.com{url}"));
    let options = Options::default().with_rewrite_url(&rewrite);
    assert_eq!(
        convert("<a href=\"/docs\">docs</a> <a href=\"javascript:x()\">run</a> <img src=\"javascript:y\" alt=\"i\">", &options).unwrap(),
        "[docs](https://e.com/docs) run"
    );
}

#[test]
fn data_tables_become_pipe_tables_without_padding() {
    assert_eq!(
        markdown(
            "<table><caption>Prices</caption><tr><th>Item</th><th>Price</th></tr><tr><td>a|b</td><td><code>x|y</code></td><td>extra</td></tr><tr></tr><tr><td>12.50</td></tr></table>"
        ),
        "Prices\n\n| Item | Price |  |\n| --- | --- | --- |\n| a\\|b | `x\\|y` | extra |\n| 12.50 |  |  |"
    );
}

#[test]
fn layout_tables_and_nested_tables_render_as_their_content() {
    assert_eq!(
        markdown("<table><tr><td>layout</td><td>cells</td></tr></table>"),
        "layout\n\ncells"
    );
    assert_eq!(
        markdown(
            "<table><tr><th>h</th></tr><tr><td><table><tr><th>in</th></tr><tr><td>x</td></tr></table><ul><li>a</li><li>b</li></ul></td></tr></table>"
        ),
        "| h |\n| --- |\n| in x a b |"
    );
}

#[test]
fn hidden_and_non_content_elements_are_left_out() {
    assert_eq!(
        markdown(
            "<head><title>T</title><style>s</style></head><p>shown</p><script>x()</script><div hidden>no</div><p aria-hidden=\"true\">no</p><noscript>no</noscript><template><p>no</p></template><svg><style>s</style><text>chart</text></svg>"
        ),
        "shown\n\nchart"
    );
}

#[test]
fn hidden_svg_and_mathml_are_left_out_too() {
    let html = "<p>Save<svg aria-hidden=\"true\"><title>floppy icon</title></svg><math hidden><mi>x</mi></math></p>";
    assert_eq!(markdown(html), "Save");
    let text = Options::default().with_format(htmlmark::Format::Text);
    assert_eq!(convert(html, &text).unwrap(), "Save");
}

#[test]
fn line_start_syntax_split_across_elements_is_escaped() {
    // A list number split over text nodes.
    for html in [
        "<p><span>1</span>. x</p>",
        "<p>1<!-- -->. x</p>",
        "<p>1<b>.</b> x</p>",
    ] {
        assert_eq!(markdown(html), "1\\. x", "{html}");
    }
    assert_eq!(markdown("<p>1<span>2</span>) x 1.5</p>"), "12\\) x 1.5");
    // Text inside a list before its first item, then an item not numbered 1.
    assert_eq!(
        markdown("<ol start=\"3\">Steps:<li>Open</li></ol>"),
        "Steps:\n\n3. Open"
    );
    // A caption written above a pipe table starts its own line.
    assert_eq!(
        markdown("<table><caption># x</caption><tr><th>h</th></tr></table>"),
        "\\# x\n\n| h |\n| --- |"
    );
    // Lines split by `<br>` that would read as a table's delimiter row.
    assert_eq!(
        markdown("<p>a | b<br>:-- | :--</p>"),
        "a | b  \n\\:-- | :--"
    );
    assert_eq!(markdown("<p>a|b<br>-:|-:</p>"), "a|b  \n\\-:|-:");
    // Vertical tabs are spaces to a CommonMark reader.
    assert_eq!(markdown("<h1>&#xb;#</h1>"), "# \\#");
}

#[test]
fn removing_emphasis_never_turns_a_link_into_an_image() {
    assert_eq!(
        markdown("<p>!<b><a href=\"https://e.com/u\">y</a></b>z</p>"),
        "\\![y](https://e.com/u)z"
    );
}

#[test]
fn emphasis_next_to_marks_and_symbols_is_left_out_when_unsure() {
    // A combining accent and a circled letter are not punctuation, but
    // CommonMark reads the circled letter as a symbol.
    assert_eq!(markdown("<p>x&#x301;<b>\"y\"</b></p>"), "x\u{301}\"y\"");
    assert_eq!(markdown("<p><b>x\u{24b6}</b>y</p>"), "x\u{24b6}y");
    assert_eq!(
        markdown("<p>\u{24b6}_\u{24b6}_\u{24b6}</p>"),
        "\u{24b6}\\_\u{24b6}\\_\u{24b6}"
    );
    // A nested emphasis that adds nothing does not keep a visible space
    // inside the closer when the line ends.
    assert_eq!(
        markdown("<p><b><i>x.</i><b>y&nbsp;<div>z</div></b></b></p>"),
        "**x.y**\u{a0}\n\n**z**"
    );
}

#[test]
fn entities_in_link_destinations_and_titles_stay_literal() {
    assert_eq!(
        markdown("<a href=\"https://e.com/a&amp;copy;b\" title=\"x &amp;amp; y\">l</a>"),
        "[l](https://e.com/a\\&copy;b \"x \\&amp; y\")"
    );
}
