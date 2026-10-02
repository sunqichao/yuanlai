use yuanlai_types::{BlockHeader, Target, U256};

/// BLK-06: the block ID, read as a big-endian integer, is at most the target.
#[must_use]
pub fn meets_target(header: &BlockHeader) -> bool {
    U256::from_be_bytes(header.id().0) <= header.target.0
}

/// `work(target) = ⌊2²⁵⁶ / (target + 1)⌋`, computed as `⌊(2²⁵⁶ − 1 − target) / (target + 1)⌋ + 1`.
///
/// Returns `None` only for `target = 2²⁵⁶ − 1`, which exceeds `POW_LIMIT` and never occurs in a
/// valid chain.
#[must_use]
pub fn block_work(target: Target) -> Option<U256> {
    let divisor = target.0.checked_add(U256::ONE)?;
    let (quotient, _) = (!target.0).checked_div_rem(divisor)?;
    quotient.checked_add(U256::ONE)
}
