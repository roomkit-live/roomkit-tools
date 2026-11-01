//! The text form of content revisions, shared by every tool that reports one.
//!
//! Tools hash content themselves; this module only spells the digest, so
//! the contracts stay free of hashing dependencies.

use std::fmt;

/// Spell a SHA-256 digest as a revision: `sha256:` and lowercase hex digits.
///
/// Tools return this string as `revision` and compare it with the
/// `expected_revision` a caller sends back.
pub fn sha256(digest: &[u8]) -> String {
    format!("sha256:{}", LowerHex(digest))
}

struct LowerHex<'a>(&'a [u8]);

impl fmt::Display for LowerHex<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::sha256;

    #[test]
    fn digests_are_spelled_as_prefixed_lowercase_hex() {
        assert_eq!(sha256(&[0x00, 0x0f, 0xab, 0xff]), "sha256:000fabff");
        assert_eq!(sha256(&[]), "sha256:");
    }
}
