//! [`Format::Text`](crate::Format::Text): the visible words, separated by
//! single spaces.

use crate::tree::Tree;
use crate::walk::{Event, walk};
use crate::{Error, Options};

pub(crate) fn render(tree: &Tree, options: &Options<'_>) -> Result<String, Error> {
    let mut text = String::new();
    walk(tree, options, |event| {
        let Event::Text(words) = event else {
            return Ok(());
        };
        for word in words.split_whitespace() {
            let separator = usize::from(!text.is_empty());
            if text.len() + separator + word.len() > options.max_output_bytes {
                return Err(Error::OutputTooLong);
            }
            if separator == 1 {
                text.push(' ');
            }
            text.push_str(word);
        }
        Ok(())
    })?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use crate::{Error, Format, Options, convert};

    fn text(html: &str, max_output_bytes: usize) -> Result<String, Error> {
        let options = Options::default()
            .with_format(Format::Text)
            .with_max_output_bytes(max_output_bytes);
        convert(html, &options)
    }

    #[test]
    fn visible_words_are_joined_by_single_spaces() {
        let html = "<head><title>T</title></head><p>Free  shipping\n</p><script>x()</script><p hidden>no</p><b>a</b>b<svg><style>s{}</style><text>chart</text></svg>";
        assert_eq!(text(html, 1024).unwrap(), "Free shipping a b chart");
    }

    #[test]
    fn the_byte_limit_is_exact() {
        let html = "<p>abc def</p>";
        assert_eq!(text(html, 7).unwrap(), "abc def");
        assert_eq!(text(html, 6), Err(Error::OutputTooLong));
    }
}
