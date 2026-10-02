//! Proof-of-work block producer for YuanLai, including delegated mining via `beneficiary`.

use core::ops::RangeInclusive;

use yuanlai_codec::Encode;
use yuanlai_crypto::{Tag, TaggedHasher};
use yuanlai_types::{BlockHeader, U256};

/// Searches `nonces` for one that satisfies the header's target (BLK-06).
///
/// Returns the solved header, or `None` if no nonce in the range works; the caller then changes
/// the timestamp or `tx_root` and continues.
#[must_use]
pub fn solve(template: &BlockHeader, nonces: RangeInclusive<u64>) -> Option<BlockHeader> {
    let mut bytes = template.encode();
    let prefix = TaggedHasher::new(Tag::Block);
    let target = template.target.0;
    for nonce in nonces {
        // The nonce is the last field of the header encoding.
        let (_, nonce_slot) = bytes.split_last_chunk_mut::<8>()?;
        *nonce_slot = nonce.to_le_bytes();
        let mut hasher = prefix.clone();
        hasher.update(&bytes);
        if U256::from_be_bytes(hasher.finalize().0) <= target {
            return Some(BlockHeader { nonce, ..*template });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use yuanlai_consensus::meets_target;
    use yuanlai_types::{Hash, Height, Network, Target, Timestamp};

    use super::*;

    fn template(target: U256) -> BlockHeader {
        BlockHeader {
            version: 1,
            chain_id: Network::Regtest.chain_id(),
            height: Height(1),
            prev_block: Hash([1; 32]),
            tx_root: Hash([2; 32]),
            timestamp: Timestamp(1_790_000_000),
            target: Target(target),
            nonce: 0,
        }
    }

    #[test]
    fn solution_agrees_with_consensus_check_and_is_the_first_in_range() {
        let t = template(Network::Regtest.pow_limit());
        let solved = solve(&t, 0..=1_000).unwrap();
        assert!(meets_target(&solved));
        for nonce in 0..solved.nonce {
            assert!(!meets_target(&BlockHeader { nonce, ..t }));
        }
    }

    #[test]
    fn exhausted_range_returns_none() {
        assert_eq!(solve(&template(U256::ZERO), 0..=100), None);
    }
}
