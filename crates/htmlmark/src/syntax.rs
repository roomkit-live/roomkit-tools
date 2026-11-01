//! Markdown spelling: escapes, code spans, link destinations and titles.

/// HTML collapses these into one space.
pub(crate) fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{c}' | '\r')
}

/// Punctuation as CommonMark's emphasis rules see it: ASCII punctuation and
/// any other character that is neither a letter, a digit nor a space.
pub(crate) fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

/// A letter or digit: next to one, emphasis markers must touch whitespace or
/// punctuation on their other side.
pub(crate) fn is_word(c: char) -> bool {
    !c.is_whitespace() && !is_punctuation(c)
}

/// The number and delimiter of text starting like an ordered list item:
/// one to nine digits, then `.` or `)`, then a space or the end.
pub(crate) fn ordinal(text: &str) -> Option<(&str, char)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let (number, rest) = text.split_at_checked(digits)?;
    let mut rest = rest.chars();
    let delimiter = rest.next().filter(|c| matches!(c, '.' | ')'))?;
    let ends = rest.next().is_none_or(is_html_space);
    ((1..=9).contains(&digits) && ends).then_some((number, delimiter))
}

/// Whether `c` at the start of a line, before `next`, could begin a heading,
/// quote, list item, rule or setext underline.
pub(crate) fn starts_block(c: char, next: Option<char>) -> bool {
    match c {
        '#' | '>' | '|' => true,
        '-' | '+' | '=' => next.is_none_or(|next| is_html_space(next) || next == c),
        _ => false,
    }
}

/// A code span for `code`, whose whitespace is collapsed as HTML shows it.
/// The fence is one backtick longer than the longest run inside.
pub(crate) fn code_span(code: &str) -> String {
    let collapsed = code
        .split(is_html_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    let fence = "`".repeat(longest_run(&collapsed, '`') + 1);
    let pad = if collapsed.starts_with('`') || collapsed.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{collapsed}{pad}{fence}")
}

/// Length of the longest run of `c` in `text`.
pub(crate) fn longest_run(text: &str, c: char) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for found in text.chars() {
        run = if found == c { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// A link destination: spaces, controls and angle brackets percent-encoded,
/// parentheses and backslashes escaped.
pub(crate) fn destination(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' ' | '<' | '>' => percent(&mut out, c),
            c if c.is_control() => percent(&mut out, c),
            c => out.push(c),
        }
    }
    out
}

/// Append `c` percent-encoded, one `%XX` per UTF-8 byte.
fn percent(out: &mut String, c: char) {
    let mut bytes = [0; 4];
    for byte in c.encode_utf8(&mut bytes).bytes() {
        out.push('%');
        for nibble in [byte >> 4, byte & 0xf] {
            out.extend(char::from_digit(u32::from(nibble), 16).map(|c| c.to_ascii_uppercase()));
        }
    }
}

/// A link title in double quotes, or nothing when `title` has no words.
pub(crate) fn title(title: Option<&str>) -> String {
    let words: Vec<&str> = title
        .unwrap_or_default()
        .split(is_html_space)
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty() {
        return String::new();
    }
    let mut out = String::from(" \"");
    for c in words.join(" ").chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}
