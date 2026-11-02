use super::super::{invalid, mime};
use crate::{BackendOutput, SearchCitation, SearchHit, SearchPayload, SearchUsage};
use roomkit_tools_core::ToolError;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Deserialize)]
struct Response {
    status: String,
    output: Vec<Item>,
    error: Option<serde_json::Value>,
    incomplete_details: Option<Details>,
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct Details {
    reason: String,
}
#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Item {
    WebSearchCall {
        id: String,
        status: String,
        action: Action,
    },
    Message {
        role: String,
        status: String,
        content: Vec<Content>,
    },
    Reasoning {},
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Search { sources: Option<Vec<Source>> },
    OpenPage {},
    FindInPage {},
}
#[derive(Deserialize)]
struct Source {
    r#type: String,
    url: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    OutputText {
        text: String,
        annotations: Vec<Annotation>,
    },
    Refusal {},
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Annotation {
    UrlCitation {
        start_index: usize,
        end_index: usize,
        url: String,
        title: String,
    },
}
#[derive(Default)]
struct Collected {
    sources: Vec<SearchHit>,
    citations: Vec<SearchCitation>,
    answer: String,
    calls: HashSet<String>,
    searches: u64,
    partial: bool,
    refused: bool,
    notices: Vec<String>,
}

pub(super) fn response(
    bytes: &[u8],
    content_type: &str,
    max_calls: u64,
) -> Result<BackendOutput, ToolError> {
    if !mime(content_type).eq_ignore_ascii_case("application/json") {
        return Err(invalid());
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if response.error.is_some() {
        return Err(ToolError::UnsupportedResource(
            "OpenAI response reported an error".into(),
        ));
    }
    let partial = match response.status.as_str() {
        "completed" => false,
        "incomplete" => true,
        _ => {
            return Err(ToolError::UnsupportedResource(
                "OpenAI response did not finish successfully".into(),
            ));
        }
    };
    let mut collected = Collected {
        partial,
        ..Collected::default()
    };
    if partial {
        let reason = response.incomplete_details.ok_or_else(invalid)?.reason;
        let notice = match reason.as_str() {
            "max_output_tokens" => "OpenAI response incomplete: output token limit reached",
            "content_filter" => "OpenAI response incomplete: content filter",
            _ => "OpenAI response incomplete: unrecognized provider reason",
        };
        collected.notices.push(notice.into());
    }
    for item in response.output {
        collected.item(item)?;
    }
    if u64::try_from(collected.calls.len()).map_err(|_| invalid())? > max_calls {
        return Err(ToolError::LimitExceeded(
            "OpenAI exceeded the configured tool-call ceiling".into(),
        ));
    }
    let usage = Some(SearchUsage {
        search_requests: Some(collected.searches),
        input_tokens: response.usage.as_ref().and_then(|u| u.input_tokens),
        output_tokens: response.usage.as_ref().and_then(|u| u.output_tokens),
    });
    let payload = if collected.searches == 0 {
        if partial {
            return Err(ToolError::LimitExceeded(
                "OpenAI response incomplete before a search was observed".into(),
            ));
        }
        SearchPayload::NotPerformed {
            reason: if collected.refused {
                "OpenAI refused without an observed search".into()
            } else {
                "OpenAI completed without an observed search action".into()
            },
        }
    } else {
        if collected.refused {
            return Err(ToolError::AccessRefused(
                "OpenAI refused to return a search answer".into(),
            ));
        }
        if collected.answer.trim().is_empty() {
            return Err(if partial {
                ToolError::LimitExceeded("OpenAI search ended before producing answer text".into())
            } else {
                ToolError::UnsupportedResource("OpenAI completed search without answer text".into())
            });
        }
        if collected.citations.is_empty() {
            collected.partial = true;
            collected
                .notices
                .push("Search was observed, but the answer has no URL citations".into());
        }
        SearchPayload::Answer {
            answer: collected.answer,
            sources: collected.sources,
            citations: collected.citations,
        }
    };
    Ok(BackendOutput {
        payload,
        partial: collected.partial,
        notices: collected.notices,
        usage,
    })
}

impl Collected {
    fn source(&mut self, url: String, title: Option<String>) -> Result<usize, ToolError> {
        if let Some(index) = self.sources.iter().position(|s| s.url == url) {
            if let Some(hit) = self.sources.get_mut(index)
                && title.is_some()
            {
                hit.title = title;
            }
            return Ok(index);
        }
        if self.sources.len() >= 100 {
            return Err(ToolError::LimitExceeded("too many OpenAI sources".into()));
        }
        let index = self.sources.len();
        self.sources.push(SearchHit {
            url,
            title,
            content: None,
            omitted_bytes: 0,
        });
        Ok(index)
    }
    fn item(&mut self, item: Item) -> Result<(), ToolError> {
        match item {
            Item::Reasoning {} => (),
            Item::WebSearchCall { id, status, action } => {
                if id.is_empty() || !self.calls.insert(id) {
                    return Err(invalid());
                }
                if status != "completed" {
                    return Err(ToolError::UnsupportedResource(
                        "OpenAI search call did not complete".into(),
                    ));
                }
                match action {
                    Action::Search { sources } => {
                        self.searches += 1;
                        for source in sources.unwrap_or_default() {
                            if source.r#type != "url" {
                                return Err(ToolError::UnsupportedResource(
                                    "OpenAI returned an unsupported source type".into(),
                                ));
                            }
                            self.source(source.url.ok_or_else(invalid)?, None)?;
                        }
                    }
                    Action::OpenPage {} | Action::FindInPage {} => (),
                }
            }
            Item::Message {
                role,
                status,
                content,
            } => {
                if role != "assistant"
                    || !(status == "completed" || (status == "incomplete" && self.partial))
                {
                    return Err(invalid());
                }
                for part in content {
                    match part {
                        Content::Refusal {} => self.refused = true,
                        Content::OutputText { text, annotations } => {
                            self.text(&text, annotations)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn text(&mut self, text: &str, annotations: Vec<Annotation>) -> Result<(), ToolError> {
        if !self.answer.is_empty() {
            self.answer.push_str("\n\n");
        }
        let base = self.answer.len();
        // OpenAI documents character spans and demonstrates Python text[start:end].
        // Convert Unicode scalar indexes to UTF-8 offsets before concatenating parts.
        let boundaries: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();
        for annotation in annotations {
            let Annotation::UrlCitation {
                start_index,
                end_index,
                url,
                title,
            } = annotation;
            if start_index >= end_index {
                return Err(invalid());
            }
            let start = *boundaries.get(start_index).ok_or_else(invalid)?;
            let end = *boundaries.get(end_index).ok_or_else(invalid)?;
            let source_index = self.source(url, Some(title))?;
            if self.citations.len() >= 1000 {
                return Err(ToolError::LimitExceeded("too many OpenAI citations".into()));
            }
            self.citations.push(SearchCitation {
                source_index,
                start: base + start,
                end: base + end,
            });
        }
        self.answer.push_str(text);
        Ok(())
    }
}
