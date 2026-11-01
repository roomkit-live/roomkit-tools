use crate::{FetchFormat, parse};
use markup5ever_rcdom::NodeData;
use reqwest::Url;
use roomkit_tools_core::Error;
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
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(SKIP.to_vec())
        .build();
    let tree = parse::parse(&source, cancel)?;
    let mut pending = vec![(Rc::clone(&tree), 0)];
    let mut nodes = 0;
    let mut text = String::new();
    let mut link_bytes = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        nodes += 1;
        if nodes > parse::MAX_NODES || depth > parse::MAX_DEPTH {
            return Err(parse::too_large());
        }
        match &node.data {
            NodeData::Element { name, attrs, .. } => {
                if SKIP.contains(&name.local.as_ref())
                    || attrs.borrow().iter().any(|a| {
                        a.name.local.as_ref() == "hidden"
                            || (a.name.local.as_ref() == "aria-hidden"
                                && a.value.as_ref() == "true")
                    })
                {
                    node.children.borrow_mut().clear();
                    continue;
                }
                attrs.borrow_mut().retain_mut(|a| {
                    if !matches!(a.name.local.as_ref(), "href" | "src") {
                        return true;
                    }
                    if let Ok(url) = base.join(&a.value)
                        && matches!(url.scheme(), "http" | "https" | "mailto")
                    {
                        link_bytes = link_bytes.saturating_add(url.as_str().len());
                        a.value = url.as_str().into();
                        return true;
                    }
                    false
                });
                if link_bytes > max_bytes {
                    return Err(Error::LimitExceeded(
                        "expanded HTML links exceed host byte limit".into(),
                    ));
                }
            }
            NodeData::Text { contents } if format == FetchFormat::Text => {
                for word in contents.borrow().split_whitespace() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(word);
                    if text.len() > max_bytes {
                        return Err(Error::LimitExceeded(
                            "converted text exceeds host byte limit".into(),
                        ));
                    }
                }
            }
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::Text { .. }
            | NodeData::Comment { .. }
            | NodeData::ProcessingInstruction { .. } => {}
        }
        for child in node.children.borrow().iter().rev() {
            pending.push((Rc::clone(child), depth + 1));
        }
    }
    if format == FetchFormat::Text {
        Ok(text)
    } else {
        Ok(converter.tree_to_markdown(&tree))
    }
}
