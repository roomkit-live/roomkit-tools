//! `ask_user`: the agent asks the person one to four questions and waits for
//! the answers. The host shows the questions and returns what the person
//! picked or wrote; a host that serves the call itself, as RoomKit's human
//! input does, reads the call's arguments with [`AskUserTool::read`].

use std::collections::HashSet;
use std::sync::Arc;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::{AgentBackend, AgentContract, AgentScope, decode, render};

const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 4;
const MAX_QUESTION_BYTES: usize = 1_024;
const MAX_HEADER_BYTES: usize = 64;
const MAX_LABEL_BYTES: usize = 256;
const MAX_DESCRIPTION_BYTES: usize = 512;
const MAX_OTHER_BYTES: usize = 4_096;

/// A choice the person may pick.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserOption {
    /// One to five words: what the user picks.
    pub label: String,
    /// What picking it means.
    #[serde(default)]
    pub description: String,
}

/// One question.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestion {
    /// A label of at most 12 characters naming the question.
    #[serde(default)]
    pub header: String,
    /// The question, ending with a question mark.
    pub question: String,
    /// The choices, most likely first; empty for a free answer.
    #[serde(default)]
    #[schemars(length(max = 4))]
    pub options: Vec<AskUserOption>,
    /// Whether the user may pick several options.
    #[serde(default)]
    pub multi_select: bool,
}

/// Ask the person one to four questions.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserInput {
    /// Independent questions, asked together.
    #[schemars(length(min = 1, max = 4))]
    pub questions: Vec<AskUserQuestion>,
}

/// What the person answered to one question.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserAnswer {
    /// The labels of the options picked.
    #[serde(default)]
    pub selected: Vec<String>,
    /// What the person wrote in their own words, if anything.
    #[serde(default)]
    pub other: Option<String>,
}

/// The person's answers, one per question, in the order asked.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserResult {
    /// One answer per question.
    pub answers: Vec<AskUserAnswer>,
}

fn invalid(message: String) -> ToolError {
    ToolError::InvalidInput(message)
}

fn within(value: &str, max: usize, name: impl FnOnce() -> String) -> Result<(), ToolError> {
    if value.len() > max {
        return Err(ToolError::LimitExceeded(format!(
            "{} exceeds {max} bytes.",
            name()
        )));
    }
    Ok(())
}

impl AskUserQuestion {
    /// The checks of the question numbered `number`, from one, in words the
    /// model can act on.
    fn check(&self, number: usize) -> Result<(), ToolError> {
        if self.question.trim().is_empty() {
            return Err(invalid(format!("Question {number} has no text.")));
        }
        within(&self.question, MAX_QUESTION_BYTES, || {
            format!("Question {number}'s text")
        })?;
        within(&self.header, MAX_HEADER_BYTES, || {
            format!("Question {number}'s header")
        })?;
        if self.options.len() > MAX_OPTIONS {
            return Err(invalid(format!(
                "Question {number} offers {} options; at most {MAX_OPTIONS}.",
                self.options.len()
            )));
        }
        let mut labels = HashSet::new();
        for option in &self.options {
            let label = option.label.trim();
            if label.is_empty() || !labels.insert(label) {
                return Err(invalid(format!(
                    "Question {number} needs distinct, non-empty option labels."
                )));
            }
            within(&option.label, MAX_LABEL_BYTES, || {
                format!("An option label of question {number}")
            })?;
            within(&option.description, MAX_DESCRIPTION_BYTES, || {
                format!("An option description of question {number}")
            })?;
        }
        Ok(())
    }
}

impl AgentContract for AskUserInput {
    fn validate(&self) -> Result<(), ToolError> {
        let count = self.questions.len();
        if count == 0 || count > MAX_QUESTIONS {
            return Err(invalid(format!(
                "ask_user takes one to {MAX_QUESTIONS} questions; {count} were given."
            )));
        }
        self.questions
            .iter()
            .enumerate()
            .try_for_each(|(index, question)| question.check(index + 1))
    }
}

impl AgentContract for AskUserResult {
    fn validate(&self) -> Result<(), ToolError> {
        if self.answers.len() > MAX_QUESTIONS {
            return Err(ToolError::LimitExceeded(format!(
                "at most {MAX_QUESTIONS} answers"
            )));
        }
        for answer in &self.answers {
            if answer.selected.len() > MAX_OPTIONS {
                return Err(ToolError::LimitExceeded(format!(
                    "at most {MAX_OPTIONS} selected options per answer"
                )));
            }
            for label in &answer.selected {
                within(label, MAX_LABEL_BYTES, || "A selected label".into())?;
            }
            if let Some(other) = &answer.other {
                within(other, MAX_OTHER_BYTES, || "An answer".into())?;
            }
        }
        Ok(())
    }
}

tool!(
    AskUserTool,
    AskUserInput,
    AskUserResult,
    ask,
    "ask_user",
    "Ask the user one to four questions and wait for the answers. \
Use it only when you are blocked on a decision that is genuinely theirs: a preference, a choice \
between valid approaches, information the project cannot tell you. Do not use it to ask for \
permission to act (approvals handle that), to ask whether to continue, or for what you can find by \
reading the project. Ask independent questions in one call. Offer two to four options when the \
answer is a choice, the most likely first, its label ending with \"(Recommended)\" when you \
recommend it; leave the options empty for a free answer. The user can always answer in their own \
words: never add an \"Other\" option. Never write a multiple-choice question as a message instead. \
If the user does not answer, continue with your best judgement and do not ask again."
);

impl AskUserTool {
    /// The questions of an `ask_user` call's arguments, checked as
    /// [`call`](Self::call) checks them, for a host that serves the call
    /// itself.
    /// # Errors
    /// What the model should fix in its call: malformed arguments, too many
    /// or empty questions, duplicate labels, oversized text.
    pub fn read(arguments: &Map<String, Value>) -> Result<AskUserInput, ToolError> {
        // Through the JSON Python's `read` decodes, under the same bound.
        let json = serde_json::to_string(arguments).map_err(|e| invalid(e.to_string()))?;
        decode(&json)
    }
}
