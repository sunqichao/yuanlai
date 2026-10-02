use crate::hash::{Hash, Tag, TaggedHasher, tagged_hash};

/// `MTH(D)` from `spec/01-serialization.md` §5 (RFC 6962 shape with tagged hashes).
#[must_use]
pub fn merkle_root<T: AsRef<[u8]>>(leaves: &[T]) -> Hash {
    match leaves {
        [] => tagged_hash(Tag::MerkleEmpty, &[]),
        [only] => tagged_hash(Tag::MerkleLeaf, only.as_ref()),
        _ => {
            let (left, right) = leaves.split_at(split_point(leaves.len()));
            let mut hasher = TaggedHasher::new(Tag::MerkleNode);
            hasher.update(merkle_root(left).as_bytes());
            hasher.update(merkle_root(right).as_bytes());
            hasher.finalize()
        }
    }
}

/// Largest power of two strictly less than `n`, for `n ≥ 2`.
fn split_point(n: usize) -> usize {
    let exponent = n.saturating_sub(1).ilog2();
    1_usize.checked_shl(exponent).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::split_point;

    #[test]
    fn split_point_is_largest_power_of_two_below_n() {
        let expected = [(2, 1), (3, 2), (4, 2), (5, 4), (8, 4), (9, 8), (17, 16)];
        for (n, k) in expected {
            assert_eq!(split_point(n), k, "n = {n}");
        }
    }
}
