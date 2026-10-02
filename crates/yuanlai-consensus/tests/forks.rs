//! Fork switching tests for `spec/02-block.md` FORK-03.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    reason = "test code: any failure should abort the test"
)]

use yuanlai_codec::BoundedBytes;
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{Chain, meets_target};
use yuanlai_crypto::public_key;
use yuanlai_types::{Block, Hash, Height, Network, PubKey, Timestamp};

fn empty_chain() -> Chain {
    let network = Network::Regtest;
    let genesis = genesis_block(network).unwrap();
    Chain::new(network.params(), &genesis).unwrap()
}

fn extend(chain: &mut Chain, beneficiary: PubKey, count: usize) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(count);
    for _ in 0..count {
        let timestamp = Timestamp(
            chain
                .min_next_timestamp()
                .0
                .max(chain.tip().timestamp.0.saturating_add(1)),
        );
        let mut block = chain
            .template(beneficiary, BoundedBytes::empty(), timestamp)
            .unwrap();
        while !meets_target(&block.header) {
            block.header.nonce = block.header.nonce.checked_add(1).unwrap();
        }
        chain.connect(&block).unwrap();
        blocks.push(block);
    }
    blocks
}

#[test]
fn reorganization_undoes_old_state_and_applies_the_replacement_branch() {
    let owner_a = PubKey(public_key(&[0x11; 32]));
    let owner_b = PubKey(public_key(&[0x22; 32]));
    let mut active = empty_chain();
    let _old_branch = extend(&mut active, owner_a, 2);
    let mut competing = empty_chain();
    let new_branch = extend(&mut competing, owner_b, 3);

    active.reorganize(Height(0), &new_branch).unwrap();

    assert_eq!(active.height(), Height(3));
    assert_eq!(active.tip().id(), competing.tip().id());
    assert_eq!(active.state().balance(&owner_a).0, 0);
    assert_eq!(active.state().balance(&owner_b).0, 30);
}

#[test]
fn invalid_replacement_branch_leaves_the_active_chain_unchanged() {
    let owner_a = PubKey(public_key(&[0x11; 32]));
    let owner_b = PubKey(public_key(&[0x22; 32]));
    let mut active = empty_chain();
    let _old_branch = extend(&mut active, owner_a, 2);
    let old_tip = active.tip().id();
    let old_balance = active.state().balance(&owner_a);

    let mut competing = empty_chain();
    let mut new_branch = extend(&mut competing, owner_b, 2);
    new_branch[1].header.prev_block = Hash([0xff; 32]);

    assert!(active.reorganize(Height(0), &new_branch).is_err());
    assert_eq!(active.tip().id(), old_tip);
    assert_eq!(active.state().balance(&owner_a), old_balance);
}
