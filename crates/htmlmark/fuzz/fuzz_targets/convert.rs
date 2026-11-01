//! Any input, any limits: no panic, and the output never exceeds its limit.

#![no_main]

use htmlmark::{Format, Options, convert};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&settings, page)) = data.split_first() else {
        return;
    };
    let html = String::from_utf8_lossy(page);
    let format = if settings & 1 == 0 {
        Format::Markdown
    } else {
        Format::Text
    };
    let max_output_bytes = usize::from(settings >> 1) * 64 + 1;
    let rewrite =
        |url: &str| (!url.starts_with("javascript:")).then(|| format!("https://e.com/{url}"));
    let options = Options::default()
        .with_format(format)
        .with_max_output_bytes(max_output_bytes)
        .with_max_nodes(5_000)
        .with_max_depth(64)
        .with_rewrite_url(&rewrite);
    if let Ok(output) = convert(&html, &options) {
        assert!(output.len() <= max_output_bytes);
    }
});
