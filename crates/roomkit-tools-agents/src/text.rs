//! What a model reads of each agent tool's result: lines, not the JSON of
//! `data`. IDs and profiles come from the host and are quoted, for the model
//! to copy them exactly; states are spelled as the schema spells them. A
//! task's result and a person's answer keep their own lines, indented under
//! the line they belong to, rather than one escaped string.

use std::fmt::Write as _;

use serde::Serialize;

use crate::{AskUserAnswer, AskUserResult, CancelResult, SendResult, TaskInfo, TaskList};

/// What lines held by a task or an answer start with, so that none of them
/// reads as a task or an answer of its own.
const INDENT: &str = "   ";

/// A state as the schema spells it.
fn spelled(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The lines of `text`, broken wherever a reader breaks a line: at `\n` and
/// `\r\n`, and at a lone `\r`, U+2028 and U+2029, which would otherwise
/// leave what follows them on an indented line's own line.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .flat_map(|line| line.split(['\r', '\u{2028}', '\u{2029}']))
}

/// A task: its states and agent on one line, then its result, if any, one
/// indented line per line.
fn task(text: &mut String, info: &TaskInfo) {
    let _written = writeln!(
        text,
        "task {:?}: {}, delivery {}; agent {:?} (profile {:?}), {}",
        info.task_id,
        spelled(info.status),
        spelled(info.delivery),
        info.agent_id,
        info.profile,
        if info.agent_available {
            "available"
        } else {
            "not available"
        }
    );
    if let Some(result) = &info.result {
        let truncated = if info.result_truncated {
            " (truncated)"
        } else {
            ""
        };
        let _written = writeln!(text, "{INDENT}result{truncated}:");
        for line in lines(result) {
            let _written = writeln!(text, "{INDENT}{line}");
        }
    }
}

/// A delegation's or a follow-up's receipt.
pub(crate) fn task_info(info: &TaskInfo) -> String {
    let mut text = String::new();
    task(&mut text, info);
    text
}

/// The tasks of the scope, numbered, and the launches still resolving.
pub(crate) fn task_list(list: &TaskList) -> String {
    let count = list.tasks.len();
    let mut text = format!("{count} task{}", plural(count));
    let pending = list.pending_launches;
    if pending > 0 {
        let _written = write!(
            text,
            ", {pending} launch{} pending",
            if pending == 1 { "" } else { "es" }
        );
    }
    text.push('\n');
    for (index, info) in list.tasks.iter().enumerate() {
        let _written = write!(text, "{}. ", index + 1);
        task(&mut text, info);
    }
    text
}

/// Whether this call cancelled the task, then the task.
pub(crate) fn cancel(result: &CancelResult) -> String {
    let mut text = String::from(if result.cancelled {
        "cancelled by this call\n"
    } else {
        "not cancelled by this call\n"
    });
    task(&mut text, &result.task);
    text
}

/// A message's receipt: the agent, the outcome and what it names.
pub(crate) fn send(result: &SendResult) -> String {
    let mut text = format!("agent {:?}: {}", result.agent_id, spelled(result.status));
    if let Some(task_id) = &result.task_id {
        let _written = write!(text, ", task {task_id:?}");
    }
    if let Some(message_id) = &result.message_id {
        let _written = write!(text, ", message {message_id:?}");
    }
    let recorded = if result.history_recorded { "" } else { "not " };
    let _written = writeln!(text, "; history {recorded}recorded");
    text
}

/// The person's answers, numbered as the questions were.
pub(crate) fn answers(result: &AskUserResult) -> String {
    if result.answers.is_empty() {
        return "no answers\n".into();
    }
    let mut text = String::new();
    for (index, answer) in result.answers.iter().enumerate() {
        let _written = write!(text, "{}. ", index + 1);
        answer_lines(&mut text, answer);
    }
    text
}

/// The labels picked, quoted, and what the person wrote, its further lines
/// indented under it.
fn answer_lines(text: &mut String, answer: &AskUserAnswer) {
    let selected: Vec<String> = answer.selected.iter().map(|l| format!("{l:?}")).collect();
    if !selected.is_empty() {
        let _written = write!(text, "selected {}", selected.join(", "));
    }
    let Some(other) = &answer.other else {
        text.push_str(if selected.is_empty() {
            "no answer\n"
        } else {
            "\n"
        });
        return;
    };
    if !selected.is_empty() {
        text.push_str("; ");
    }
    let mut wrote = lines(other);
    let _written = writeln!(text, "wrote: {}", wrote.next().unwrap_or_default());
    for line in wrote {
        let _written = writeln!(text, "{INDENT}{line}");
    }
}
