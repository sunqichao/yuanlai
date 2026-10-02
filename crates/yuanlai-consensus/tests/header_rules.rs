//! Rule-by-rule tests for `spec/02-block.md`, run on regtest so that mining is instant.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    reason = "test code: any failure should abort the test"
)]

use yuanlai_consensus::policy::is_too_far_in_future;
use yuanlai_consensus::{HeaderError, HeaderTree, InsertOutcome, TreeError, meets_target};
use yuanlai_types::params::{
    MAX_FUTURE_DRIFT, RETARGET_INTERVAL, TARGET_BLOCK_SPACING, TIMEWARP_GRACE,
};
use yuanlai_types::{BlockHeader, ChainParams, Hash, Height, Network, Target, Timestamp, U256};

const GENESIS_TIME: u64 = 1_790_000_000;

fn regtest() -> ChainParams {
    Network::Regtest.params()
}

fn solve(mut header: BlockHeader) -> BlockHeader {
    while !meets_target(&header) {
        header.nonce = header.nonce.checked_add(1).unwrap();
    }
    header
}

/// A header that fails BLK-06 for its own target.
fn unsolve(mut header: BlockHeader) -> BlockHeader {
    while meets_target(&header) {
        header.nonce = header.nonce.checked_add(1).unwrap();
    }
    header
}

fn genesis(chain: &ChainParams) -> BlockHeader {
    solve(BlockHeader {
        version: 1,
        chain_id: chain.chain_id,
        height: Height(0),
        prev_block: Hash::ZERO,
        tx_root: Hash([0xEE; 32]),
        timestamp: Timestamp(GENESIS_TIME),
        target: Target(chain.pow_limit),
        nonce: 0,
    })
}

fn new_tree() -> HeaderTree {
    let chain = regtest();
    HeaderTree::new(chain, genesis(&chain)).unwrap()
}

/// Mines one block on the best tip at `timestamp`.
fn mine_next(tree: &HeaderTree, timestamp: u64) -> BlockHeader {
    solve(
        tree.next_template(Hash::ZERO, Timestamp(timestamp))
            .unwrap(),
    )
}

/// Mines `n` blocks on the best tip, `spacing` seconds apart. Returns the last timestamp.
fn extend(tree: &mut HeaderTree, n: u64, spacing: u64) -> u64 {
    let mut ts = tree.best_header().unwrap().timestamp.0;
    for _ in 0..n {
        ts = ts.checked_add(spacing).unwrap();
        let header = mine_next(tree, ts);
        assert!(matches!(
            tree.insert(header),
            Ok(InsertOutcome::NewBest { .. })
        ));
    }
    ts
}

fn rule(result: Result<InsertOutcome, TreeError>) -> &'static str {
    match result {
        Err(TreeError::Invalid(e)) => e.rule_id(),
        Err(TreeError::UnknownParent(_)) => "BLK-04",
        other => panic!("expected a rule violation, got {other:?}"),
    }
}

#[test]
fn genesis_rules() {
    let chain = regtest();
    let good = genesis(&chain);
    assert!(HeaderTree::new(chain, good).is_ok());

    let cases = [
        (
            BlockHeader {
                height: Height(1),
                ..good
            },
            "GENESIS",
        ),
        (
            BlockHeader {
                prev_block: Hash([1; 32]),
                ..good
            },
            "GENESIS",
        ),
        (
            BlockHeader {
                target: Target(U256::ONE),
                ..good
            },
            "BLK-05",
        ),
        (BlockHeader { version: 2, ..good }, "BLK-02"),
        (
            BlockHeader {
                chain_id: Network::Mainnet.chain_id(),
                ..good
            },
            "BLK-03",
        ),
        (unsolve(good), "BLK-06"),
    ];
    // Each earlier rule is checked before BLK-06, so the modified headers need no re-mining.
    for (header, expected) in cases {
        match HeaderTree::new(chain, header) {
            Err(TreeError::Invalid(e)) => assert_eq!(e.rule_id(), expected),
            other => panic!("{expected}: {other:?}"),
        }
    }
}

#[test]
fn extending_the_best_chain() {
    let mut tree = new_tree();
    let before = tree.best_id();
    let header = mine_next(&tree, GENESIS_TIME + 60);
    assert_eq!(
        tree.insert(header),
        Ok(InsertOutcome::NewBest {
            previous: before,
            fork_point: before
        })
    );
    assert_eq!(tree.insert(header), Ok(InsertOutcome::AlreadyKnown));
    assert_eq!(tree.best_id(), header.id());
    assert_eq!(tree.len(), 2);
}

#[test]
fn blk_02_version() {
    let mut tree = new_tree();
    let h = solve(BlockHeader {
        version: 2,
        ..mine_next(&tree, GENESIS_TIME + 60)
    });
    assert_eq!(rule(tree.insert(h)), "BLK-02");
}

#[test]
fn blk_03_chain_id() {
    let mut tree = new_tree();
    let h = solve(BlockHeader {
        chain_id: Network::Testnet.chain_id(),
        ..mine_next(&tree, GENESIS_TIME + 60)
    });
    assert_eq!(rule(tree.insert(h)), "BLK-03");
}

#[test]
fn blk_04_parent_and_height() {
    let mut tree = new_tree();
    let good = mine_next(&tree, GENESIS_TIME + 60);
    let orphan = solve(BlockHeader {
        prev_block: Hash([9; 32]),
        ..good
    });
    assert_eq!(
        tree.insert(orphan),
        Err(TreeError::UnknownParent(Hash([9; 32])))
    );
    let wrong_height = solve(BlockHeader {
        height: Height(2),
        ..good
    });
    assert!(matches!(
        tree.insert(wrong_height),
        Err(TreeError::Invalid(HeaderError::HeightMismatch { .. }))
    ));
}

#[test]
fn blk_05_target() {
    let mut tree = new_tree();
    let good = mine_next(&tree, GENESIS_TIME + 60);
    let (harder, _) = good
        .target
        .0
        .div_rem_u64(core::num::NonZeroU64::new(2).unwrap());
    let h = solve(BlockHeader {
        target: Target(harder),
        ..good
    });
    assert_eq!(rule(tree.insert(h)), "BLK-05");
}

#[test]
fn blk_06_proof_of_work() {
    let mut tree = new_tree();
    let h = unsolve(mine_next(&tree, GENESIS_TIME + 60));
    assert_eq!(rule(tree.insert(h)), "BLK-06");
}

#[test]
fn blk_07_timestamp_must_exceed_median_time_past() {
    let mut tree = new_tree();
    extend(&mut tree, 11, 60);
    // Last 11 timestamps are GENESIS+60 … GENESIS+660; median is GENESIS+360.
    let median = GENESIS_TIME + 360;
    let at_median = mine_next(&tree, median);
    assert_eq!(rule(tree.insert(at_median)), "BLK-07");
    let above = mine_next(&tree, median + 1);
    assert!(tree.insert(above).is_ok());
}

#[test]
fn blk_08_time_warp_at_retarget_boundary() {
    let mut tree = new_tree();
    extend(&mut tree, RETARGET_INTERVAL - 2, 60);
    // Parent of the boundary block jumps far ahead; the median stays low.
    let parent_ts = extend(&mut tree, 1, 100_000);
    assert_eq!(
        tree.best_header().unwrap().height,
        Height(RETARGET_INTERVAL - 1)
    );

    let too_early = mine_next(&tree, parent_ts - TIMEWARP_GRACE - 1);
    assert_eq!(rule(tree.insert(too_early)), "BLK-08");
    let ok = mine_next(&tree, parent_ts - TIMEWARP_GRACE);
    assert!(tree.insert(ok).is_ok());
}

#[test]
fn blk_08_does_not_apply_off_boundary() {
    let mut tree = new_tree();
    extend(&mut tree, 11, 60);
    let parent_ts = extend(&mut tree, 1, 100_000);
    let h = mine_next(&tree, parent_ts - TIMEWARP_GRACE - 1);
    assert!(tree.insert(h).is_ok());
}

#[test]
fn retarget_follows_actual_timespan() {
    let mut tree = new_tree();
    let limit = regtest().pow_limit;
    // Blocks 1..=119 at half the target spacing.
    extend(
        &mut tree,
        2 * RETARGET_INTERVAL - 1,
        TARGET_BLOCK_SPACING / 2,
    );
    assert_eq!(tree.best_header().unwrap().target, Target(limit));

    let boundary = mine_next(&tree, tree.best_header().unwrap().timestamp.0 + 30);
    let (half, _) = limit.div_rem_u64(core::num::NonZeroU64::new(2).unwrap());
    assert_eq!(boundary.target, Target(half));
    assert!(tree.insert(boundary).is_ok());
}

#[test]
fn fork_choice_prefers_work_then_first_seen() {
    let mut tree = new_tree();
    extend(&mut tree, 3, 60);
    let fork_base = tree.best_id();
    let base_ts = tree.best_header().unwrap().timestamp.0;

    let a1 = mine_next(&tree, base_ts + 60);
    let b1 = mine_next(&tree, base_ts + 61);
    assert!(matches!(tree.insert(a1), Ok(InsertOutcome::NewBest { .. })));
    // Equal work: the first-seen tip stays (FORK-02).
    assert_eq!(tree.insert(b1), Ok(InsertOutcome::SideBranch));
    assert_eq!(tree.best_id(), a1.id());

    // Extending the side branch makes it heavier (FORK-01) and reports the fork point.
    let b2 = solve(BlockHeader {
        height: Height(b1.height.0 + 1),
        prev_block: b1.id(),
        timestamp: Timestamp(b1.timestamp.0 + 60),
        ..b1
    });
    assert_eq!(
        tree.insert(b2),
        Ok(InsertOutcome::NewBest {
            previous: a1.id(),
            fork_point: fork_base
        })
    );
    assert_eq!(tree.best_id(), b2.id());
    assert_eq!(tree.common_ancestor(a1.id(), b2.id()), Some(fork_base));
    assert_eq!(tree.headers_after(fork_base, b2.id()), Some(vec![b1, b2]));
    assert_eq!(tree.headers_after(b2.id(), a1.id()), None);
}

#[test]
fn future_drift_policy() {
    let h = BlockHeader {
        version: 1,
        chain_id: Hash::ZERO,
        height: Height(1),
        prev_block: Hash::ZERO,
        tx_root: Hash::ZERO,
        timestamp: Timestamp(1_000 + MAX_FUTURE_DRIFT),
        target: Target(U256::MAX),
        nonce: 0,
    };
    assert!(!is_too_far_in_future(&h, Timestamp(1_000)));
    assert!(is_too_far_in_future(&h, Timestamp(999)));
}
