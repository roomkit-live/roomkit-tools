use std::fmt::Write as _;

use roomkit_tools_core::{Error, Output};

use crate::extract::{Control, SourceOutline};
use crate::{CodeLanguage, CodeOutlineInput, CodeOutlineLimits, CodeOutlinePage};

pub(crate) fn render(
    path: String,
    language: CodeLanguage,
    source: SourceOutline,
    input: &CodeOutlineInput,
    limits: CodeOutlineLimits,
    control: &Control,
) -> Result<Output<CodeOutlinePage>, Error> {
    let total = source.symbols.len();
    let offset = input.offset.get();
    if offset > total && !(offset == 1 && total == 0) {
        return Err(Error::InvalidInput(
            "offset is beyond the last symbol".into(),
        ));
    }
    let count = input
        .limit
        .map_or(limits.max_symbols, |n| n.get().min(limits.max_symbols));
    let mut output = Output::new(
        CodeOutlinePage {
            path,
            language,
            revision: source.revision,
            symbols: Vec::new(),
            total_symbols: total,
            syntax_errors: source.syntax_errors,
            truncated: false,
            next_offset: None,
        },
        format!(
            "{language:?} outline; syntax_errors={}\n",
            source.syntax_errors
        ),
    );
    for symbol in source.symbols.into_iter().skip(offset - 1) {
        control.check()?;
        let index = symbol.index;
        let text_len = output.text.len();
        let _ = writeln!(
            output.text,
            "{}: {:?} {:?} [{}-{}] parent={:?} signature_truncated={}\n{}",
            symbol.index,
            symbol.kind,
            symbol.name,
            symbol.start_line,
            symbol.end_line,
            symbol.parent,
            symbol.signature_truncated,
            symbol.signature
        );
        output.data.symbols.push(symbol);
        output.data.truncated = index < total;
        output.data.next_offset = (index < total).then_some(index + 1);
        if output.data.symbols.len() > count || size(&output)? > limits.max_output_bytes {
            output.data.symbols.pop();
            output.text.truncate(text_len);
            if output.data.symbols.is_empty() {
                return Err(Error::LimitExceeded(
                    "one symbol exceeds max_output_bytes".into(),
                ));
            }
            output.data.truncated = true;
            output.data.next_offset = Some(index);
            break;
        }
    }
    if size(&output)? > limits.max_output_bytes {
        return Err(Error::LimitExceeded(
            "outline output exceeds max_output_bytes".into(),
        ));
    }
    control.check()?;
    Ok(output)
}

fn size(output: &Output<CodeOutlinePage>) -> Result<usize, Error> {
    serde_json::to_vec(output)
        .map(|value| value.len())
        .map_err(|_| Error::Unsupported("outline output is not serializable".into()))
}
