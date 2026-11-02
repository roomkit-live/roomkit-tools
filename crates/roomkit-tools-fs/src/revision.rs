use roomkit_tools_core::ToolError;
use sha2::{Digest, Sha256};

pub(crate) fn finish(hash: Sha256) -> String {
    roomkit_tools_core::revision::sha256(&hash.finalize())
}

pub(crate) fn of(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    finish(hash)
}

pub(crate) fn check(expected: Option<&str>, bytes: Option<&[u8]>) -> Result<(), ToolError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if !expected.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(ToolError::InvalidInput(
            "expected_revision must be a sha256 revision returned by read, write or edit".into(),
        ));
    }
    if bytes.is_none_or(|bytes| of(bytes) != expected) {
        return Err(ToolError::AccessRefused(
            "file does not match expected_revision; read it again before changing it".into(),
        ));
    }
    Ok(())
}
