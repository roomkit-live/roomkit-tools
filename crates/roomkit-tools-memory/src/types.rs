use roomkit_tools_core::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) const MAX_NOTE: usize = 32 * 1024;
pub(crate) const MAX_RESULTS: usize = 256;

/// Arguments for the selected operation. The host supplies the namespace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryInput {
    /// One of search, read, save or forget.
    #[schemars(with = "Action")]
    pub action: String,
    /// Whitespace-separated search terms. Omit to list the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// A lowercase ASCII note ID (letters, digits and hyphens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Single-line title for save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Markdown body for save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Observed revision for replacement or removal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
#[expect(dead_code, reason = "The schema advertises the accepted action names")]
enum Action {
    Search,
    Read,
    Save,
    Forget,
}

pub(crate) fn invalid(message: &str) -> ToolError {
    ToolError::InvalidInput(message.to_owned())
}

pub(crate) fn valid_id(id: &str) -> bool {
    // Device names stay reserved with a .md extension on Windows.
    let device = matches!(id, "con" | "prn" | "aux" | "nul")
        || id
            .strip_prefix("com")
            .or_else(|| id.strip_prefix("lpt"))
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']));
    id != "memory"
        && !device
        && (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn valid_title(title: &str) -> bool {
    !title.trim().is_empty() && title.len() <= 160 && !title.chars().any(char::is_control)
}
fn valid_revision(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= MAX_NOTE && !s.contains('\0')
}

impl MemoryInput {
    /// Check field combinations and bounds before contacting a backend.
    /// # Errors
    /// Refuses unknown operations, missing arguments and invalid note metadata.
    pub(crate) fn validate(&self) -> Result<(), ToolError> {
        let has_id = self.id.as_deref().is_some_and(valid_id);
        let has_body = self.title.is_some() || self.text.is_some();
        let valid = match self.action.as_str() {
            "search" => {
                self.id.is_none()
                    && !has_body
                    && self.revision.is_none()
                    && self.query.as_ref().is_none_or(|s| s.len() <= 512)
            }
            "read" => has_id && !has_body && self.revision.is_none() && self.query.is_none(),
            "save" => {
                has_id
                    && self.query.is_none()
                    && self.title.as_deref().is_some_and(valid_title)
                    && self.text.as_deref().is_some_and(valid_text)
                    && self.revision.as_deref().is_none_or(valid_revision)
            }
            "forget" => {
                has_id
                    && !has_body
                    && self.query.is_none()
                    && self.revision.as_deref().is_some_and(valid_revision)
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(invalid(
                "Invalid memory action or arguments; read the tool schema",
            ))
        }
    }
}

/// One entry. Searches and mutation receipts omit the body.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryNote {
    /// Stable note ID.
    pub id: String,
    /// Human-readable subject.
    pub title: String,
    /// Backend revision for conditional writes.
    pub revision: String,
    /// Body returned only by read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Notes matching a search or the receipt for one operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryResult {
    /// Matching note metadata or a single read or mutation receipt.
    pub notes: Vec<MemoryNote>,
}

impl MemoryResult {
    /// Check that a backend returned bounded data belonging to the requested operation.
    /// # Errors
    /// Refuses incomplete, oversized or unrelated records.
    pub(crate) fn validate(&self, input: &MemoryInput) -> Result<(), ToolError> {
        let single = input.action != "search";
        if self.notes.len() > MAX_RESULTS || (single && self.notes.len() != 1) {
            return Err(invalid("Unexpected number of project notes"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for note in &self.notes {
            if !valid_id(&note.id)
                || !valid_title(&note.title)
                || !valid_revision(&note.revision)
                || !ids.insert(&note.id)
                || (single && Some(&note.id) != input.id.as_ref())
                || (input.action == "read") != note.text.is_some()
                || note.text.as_deref().is_some_and(|s| !valid_text(s))
            {
                return Err(invalid("Invalid project note in backend reply"));
            }
        }
        Ok(())
    }
}
