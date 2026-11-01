//! Ordering of the sequential identifiers tools issue, such as `plan-2`.

/// Sort key giving the creation order of sequential identifiers that share a
/// prefix and have no leading zeros, such as `plan-2` and `plan-10`: a shorter
/// identifier was created earlier, and identifiers of one length compare as
/// text.
pub fn creation_order(id: &str) -> (usize, &str) {
    (id.len(), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_sort_in_creation_order_past_nine() {
        let mut ids = ["p10", "p9", "p2", "p100", "p11", "p1"];
        ids.sort_by_key(|id| creation_order(id));
        assert_eq!(ids, ["p1", "p2", "p9", "p10", "p11", "p100"]);
    }
}
