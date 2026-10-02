use core::num::NonZeroU64;

use yuanlai_types::params::{MAX_RETARGET_FACTOR, RETARGET_INTERVAL, TARGET_BLOCK_SPACING};
use yuanlai_types::{Height, Target, U256};

use crate::{Ancestry, ContextError};

const EXPECTED_TIMESPAN: u64 = RETARGET_INTERVAL * TARGET_BLOCK_SPACING;
const MIN_TIMESPAN: u64 = EXPECTED_TIMESPAN / MAX_RETARGET_FACTOR;
const MAX_TIMESPAN: u64 = EXPECTED_TIMESPAN * MAX_RETARGET_FACTOR;
const _: () = assert!(EXPECTED_TIMESPAN > 0);
// Makes the `clamp` below statically panic-free.
const _: () = assert!(MIN_TIMESPAN <= MAX_TIMESPAN);
const EXPECTED_TIMESPAN_NZ: NonZeroU64 = match NonZeroU64::new(EXPECTED_TIMESPAN) {
    Some(v) => v,
    None => NonZeroU64::MIN,
};

/// `NextTarget(P)` from `spec/02-block.md` §4, for the block at `next_height` whose parent has
/// `parent_target`. `ancestry` must describe the branch ending at the parent.
///
/// # Errors
///
/// Returns [`ContextError::MissingAncestor`] if a required timestamp is not available.
pub fn next_target<A: Ancestry>(
    next_height: Height,
    parent_target: Target,
    pow_limit: U256,
    ancestry: &A,
) -> Result<Target, ContextError> {
    let h = next_height.0;
    let is_boundary = h.checked_rem(RETARGET_INTERVAL) == Some(0);
    let first_period_complete = h >= RETARGET_INTERVAL.saturating_mul(2);
    if !is_boundary || !first_period_complete {
        return Ok(parent_target);
    }

    // Exactly RETARGET_INTERVAL spacings: from block h−I−1 to block h−1. h ≥ 2I makes both
    // subtractions exact.
    let last_height = Height(h.saturating_sub(1));
    let first_height = Height(h.saturating_sub(RETARGET_INTERVAL).saturating_sub(1));
    let last = ancestry
        .timestamp_at(last_height)
        .ok_or(ContextError::MissingAncestor(last_height))?;
    let first = ancestry
        .timestamp_at(first_height)
        .ok_or(ContextError::MissingAncestor(first_height))?;

    // Timestamps may decrease (BLK-07 only bounds them by the median), so a negative span is
    // possible and is clamped to the minimum.
    let actual = if last.0 <= first.0 {
        MIN_TIMESPAN
    } else {
        last.0
            .saturating_sub(first.0)
            .clamp(MIN_TIMESPAN, MAX_TIMESPAN)
    };

    // Split P.target = q·E + r so that every intermediate value provably stays below 2²⁵⁶ for
    // targets up to 2²⁵³ (see spec §4). Saturation only matters for an out-of-range parent,
    // whose result is clamped to the limit anyway.
    let (q, r) = parent_target.0.div_rem_u64(EXPECTED_TIMESPAN_NZ);
    let high = q.checked_mul_u64(actual).unwrap_or(U256::MAX);
    let low = r
        .checked_mul(actual)
        .and_then(|v| v.checked_div(EXPECTED_TIMESPAN))
        .unwrap_or(0);
    let next = high.checked_add(U256::from_u64(low)).unwrap_or(U256::MAX);
    Ok(Target(next.max(U256::ONE).min(pow_limit)))
}
