//! Markdown spelling: escapes, code spans, link destinations and titles.

/// HTML collapses these into one space; a vertical tab is a space to a
/// CommonMark reader and shows nothing in a page, so it collapses too.
pub(crate) fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

/// How CommonMark's emphasis rules see a character, as far as the standard
/// library can tell. CommonMark punctuation is Unicode's P and S categories,
/// which Rust does not expose: outside ASCII, a character that is neither a
/// letter, a digit nor a space may be punctuation (`«`, `’`) or not (a
/// combining accent, a joiner), and a few symbols count as letters (`Ⓐ`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punctuation,
    Unsure,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_ascii_punctuation() {
        Class::Punctuation
    } else if c.is_alphanumeric() && !is_letter_symbol(c) {
        Class::Word
    } else {
        Class::Unsure
    }
}

/// Symbols (category So) that Unicode also marks alphabetic: circled and
/// squared Latin letters.
fn is_letter_symbol(c: char) -> bool {
    matches!(c, '\u{24b6}'..='\u{24e9}' | '\u{1f130}'..='\u{1f149}' | '\u{1f150}'..='\u{1f169}' | '\u{1f170}'..='\u{1f189}')
}

/// A letter or digit for certain.
pub(crate) fn is_word(c: char) -> bool {
    class(c) == Class::Word
}

/// Whether CommonMark may count `c` as a letter or digit.
pub(crate) fn may_be_word(c: char) -> bool {
    matches!(class(c), Class::Word | Class::Unsure)
}

/// Whether CommonMark may count `c` as punctuation.
pub(crate) fn may_be_punctuation(c: char) -> bool {
    matches!(class(c), Class::Punctuation | Class::Unsure)
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

/// Whether `text` starts with what CommonMark reads as an entity or numeric
/// character reference, such as `&copy;` or `&#169;`.
pub(crate) fn starts_entity(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('&') else {
        return false;
    };
    let (body, digits): (&str, fn(&char) -> bool) = match rest.strip_prefix('#') {
        Some(number) => match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, char::is_ascii_hexdigit),
            None => (number, char::is_ascii_digit),
        },
        None => (rest, char::is_ascii_alphanumeric),
    };
    let name = body.chars().take_while(digits).count();
    (1..=32).contains(&name) && body.get(name..).is_some_and(|after| after.starts_with(';'))
}

/// A link destination: spaces, controls and angle brackets percent-encoded,
/// parentheses, backslashes and entity-like `&` escaped.
pub(crate) fn destination(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for (index, c) in url.char_indices() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '&' if url.get(index..).is_some_and(starts_entity) => out.push_str("\\&"),
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
    let joined = words.join(" ");
    for (index, c) in joined.char_indices() {
        let entity = c == '&' && joined.get(index..).is_some_and(starts_entity);
        if matches!(c, '"' | '\\') || entity {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}
