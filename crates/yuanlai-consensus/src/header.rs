use yuanlai_types::params::{HEADER_VERSION, MTP_WINDOW, RETARGET_INTERVAL, TIMEWARP_GRACE};
use yuanlai_types::{BlockHeader, ChainParams, Hash, Height, Target, Timestamp};

use crate::{Ancestry, ContextError, HeaderError, median_time_past, meets_target, next_target};

/// Checks BLK-02 through BLK-08 for `header` on top of `parent`.
///
/// `ancestry` must describe the branch ending at `parent` (inclusive). Transaction rules
/// (BLK-09 onward) are checked separately once the block body is available.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn validate_header<A: Ancestry>(
    header: &BlockHeader,
    parent: &BlockHeader,
    chain: &ChainParams,
    ancestry: &A,
) -> Result<(), HeaderError> {
    check_version_and_chain(header, chain.chain_id)?;

    let expected_height = parent
        .height
        .checked_add(1)
        .ok_or(HeaderError::HeightMismatch {
            expected: parent.height,
            found: header.height,
        })?;
    if header.height != expected_height {
        return Err(HeaderError::HeightMismatch {
            expected: expected_height,
            found: header.height,
        });
    }
    if header.prev_block != parent.id() {
        return Err(HeaderError::ParentMismatch);
    }

    let expected_target = next_target(header.height, parent.target, chain.pow_limit, ancestry)?;
    if header.target != expected_target {
        return Err(HeaderError::WrongTarget {
            expected: expected_target,
            found: header.target,
        });
    }

    if !meets_target(header) {
        return Err(HeaderError::InsufficientWork);
    }

    let median = parent_median_time_past(parent.height, ancestry)?;
    if header.timestamp <= median {
        return Err(HeaderError::TimestampNotAfterMedian {
            median,
            found: header.timestamp,
        });
    }

    let at_boundary = header.height.0.checked_rem(RETARGET_INTERVAL) == Some(0);
    if at_boundary && header.timestamp.0 < parent.timestamp.0.saturating_sub(TIMEWARP_GRACE) {
        return Err(HeaderError::TimeWarp);
    }

    Ok(())
}

/// Checks the genesis header rules of `spec/02-block.md` §6 that do not depend on its body.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn validate_genesis_header(
    header: &BlockHeader,
    chain: &ChainParams,
) -> Result<(), HeaderError> {
    check_version_and_chain(header, chain.chain_id)?;
    if header.height != Height(0) || header.prev_block != Hash::ZERO {
        return Err(HeaderError::InvalidGenesis);
    }
    if header.target != Target(chain.pow_limit) {
        return Err(HeaderError::WrongTarget {
            expected: Target(chain.pow_limit),
            found: header.target,
        });
    }
    if !meets_target(header) {
        return Err(HeaderError::InsufficientWork);
    }
    Ok(())
}

fn check_version_and_chain(header: &BlockHeader, chain_id: Hash) -> Result<(), HeaderError> {
    if header.version != HEADER_VERSION {
        return Err(HeaderError::UnsupportedVersion(header.version));
    }
    if header.chain_id != chain_id {
        return Err(HeaderError::WrongChain);
    }
    Ok(())
}

/// Median of the timestamps of up to `MTP_WINDOW` blocks ending at `parent_height`.
fn parent_median_time_past<A: Ancestry>(
    parent_height: Height,
    ancestry: &A,
) -> Result<Timestamp, ContextError> {
    let mut window = Vec::with_capacity(MTP_WINDOW);
    let mut height = Some(parent_height);
    while let Some(h) = height {
        if window.len() >= MTP_WINDOW {
            break;
        }
        window.push(
            ancestry
                .timestamp_at(h)
                .ok_or(ContextError::MissingAncestor(h))?,
        );
        height = h.0.checked_sub(1).map(Height);
    }
    median_time_past(&window).ok_or(ContextError::MissingAncestor(parent_height))
}
