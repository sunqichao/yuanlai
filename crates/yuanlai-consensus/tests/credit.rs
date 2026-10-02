//! Tests for the credit state machine (`spec/04-credit.md`) and block body rules (`spec/02-block.md`).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test code: any failure should abort the test"
)]

use proptest::prelude::*;
use yuanlai_codec::BoundedBytes;
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{BlockError, Chain, ChainState, assemble, meets_target, validate_genesis};
use yuanlai_types::params::{BALANCE_CAP, BLOCK_REWARD, COINBASE_MATURITY, EXPIRY_BLOCKS};
use yuanlai_types::{
    Block, Coinbase, Credit, Hash, Height, Network, OutPoint, PubKey, RecordCommitment, Source,
    Timestamp, Tx,
};

fn key(seed: u8) -> PubKey {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    PubKey(signing.verifying_key().to_bytes())
}

fn new_chain() -> Chain {
    let network = Network::Regtest;
    Chain::new(network.params(), &genesis_block(network).unwrap()).unwrap()
}

fn solve(mut block: Block) -> Block {
    while !meets_target(&block.header) {
        block.header.nonce += 1;
    }
    block
}

fn next_timestamp(chain: &Chain) -> Timestamp {
    Timestamp(
        chain
            .min_next_timestamp()
            .0
            .max(chain.tip().timestamp.0 + 60),
    )
}

fn template(chain: &Chain, beneficiary: PubKey) -> Block {
    chain
        .template(beneficiary, BoundedBytes::empty(), next_timestamp(chain))
        .unwrap()
}

fn mine(chain: &mut Chain, beneficiary: PubKey) {
    let block = solve(template(chain, beneficiary));
    chain.connect(&block).unwrap();
}

/// Replaces the transactions of a template, recomputes `tx_root` and re-mines.
fn with_txs(block: &Block, txs: Vec<Tx>) -> Block {
    solve(assemble(block.header, txs).unwrap())
}

fn rule(chain: &mut Chain, block: &Block) -> &'static str {
    chain.connect(block).unwrap_err().rule_id()
}

fn system(block: &Block) -> (RecordCommitment, RecordCommitment, Coinbase) {
    match block.txs.as_slice() {
        [Tx::Expire(e), Tx::AutoCancel(a), Tx::Coinbase(c)] => (*e, *a, c.clone()),
        other => panic!("unexpected layout {other:?}"),
    }
}

#[test]
fn genesis_is_valid_and_mints_nothing() {
    for network in [Network::Devnet, Network::Regtest] {
        let chain = Chain::new(network.params(), &genesis_block(network).unwrap()).unwrap();
        assert_eq!(chain.height(), Height(0));
        assert_eq!(chain.state().totals().minted, Credit::ZERO);
    }
    assert!(genesis_block(Network::Mainnet).is_none());
}

#[test]
fn genesis_body_rules() {
    let chain = Network::Regtest.params();
    let genesis = genesis_block(Network::Regtest).unwrap();
    let Tx::Coinbase(cb) = &genesis.txs.as_slice()[0] else {
        panic!()
    };

    let paying = Tx::Coinbase(Coinbase {
        beneficiary: key(1),
        ..cb.clone()
    });
    let two = vec![
        genesis.txs.as_slice()[0].clone(),
        Tx::AutoCancel(RecordCommitment {
            height: Height(0),
            count: 0,
            records_root: Hash::ZERO,
        }),
    ];
    for txs in [vec![paying], two] {
        let block = with_txs(&genesis, txs);
        assert_eq!(
            validate_genesis(&block, &chain).unwrap_err().rule_id(),
            "GENESIS"
        );
    }
}

#[test]
fn coinbase_creates_immature_utxo_with_expiry() {
    let mut chain = new_chain();
    let miner = key(1);
    mine(&mut chain, miner);

    let (outpoint, utxo) = chain.state().utxos().next().map(|(o, u)| (*o, *u)).unwrap();
    assert_eq!(outpoint.index, 0);
    assert_eq!(utxo.owner, miner);
    assert_eq!(utxo.amount, BLOCK_REWARD);
    assert_eq!(utxo.source, Source::Mined);
    assert_eq!(utxo.acquired_height, Height(1));
    assert_eq!(utxo.spendable_from, Height(1 + COINBASE_MATURITY));
    assert_eq!(
        utxo.expire_height,
        Height(1 + COINBASE_MATURITY + EXPIRY_BLOCKS)
    );
    assert!(!utxo.is_spendable_at(Height(COINBASE_MATURITY)));
    assert!(utxo.is_spendable_at(Height(1 + COINBASE_MATURITY)));
    assert!(!utxo.is_spendable_at(utxo.expire_height));
    assert_eq!(chain.state().balance(&miner), BLOCK_REWARD);
}

#[test]
fn balance_cap_limits_coinbase_without_carry_over() {
    let mut chain = new_chain();
    let miner = key(1);
    let blocks_to_cap = BALANCE_CAP.0 / BLOCK_REWARD.0;
    for _ in 0..blocks_to_cap + 5 {
        mine(&mut chain, miner);
    }
    assert_eq!(chain.state().balance(&miner), BALANCE_CAP);
    assert_eq!(
        chain.state().utxos().count(),
        usize::try_from(blocks_to_cap).unwrap()
    );
    assert_eq!(chain.state().totals().minted, BALANCE_CAP);
    chain.state().audit(chain.height()).unwrap();
}

#[test]
fn expiry_removes_utxo_records_it_and_frees_cap_in_same_block() {
    let mut chain = new_chain();
    let miner = key(1);
    let blocks_to_cap = BALANCE_CAP.0 / BLOCK_REWARD.0;
    for _ in 0..blocks_to_cap {
        mine(&mut chain, miner);
    }
    // Other miners until the first reward (height 1) expires.
    let first_expiry = Height(1 + COINBASE_MATURITY + EXPIRY_BLOCKS);
    while chain.height().0 + 1 < first_expiry.0 {
        mine(&mut chain, key(2));
    }
    assert_eq!(chain.state().balance(&miner), BALANCE_CAP);

    let block = solve(template(&chain, miner));
    let (expire, _, _) = system(&block);
    assert_eq!(expire.count, 1);
    chain.connect(&block).unwrap();

    let delta = chain.delta_at(first_expiry).unwrap();
    assert_eq!(delta.expired.len(), 1);
    let record = delta.expired[0];
    assert_eq!(record.utxo.owner, miner);
    assert_eq!(record.utxo.acquired_height, Height(1));
    assert_eq!(record.utxo.expire_height, first_expiry);
    // The freed room is used by this block's Coinbase: Expire runs before Coinbase.
    assert_eq!(chain.state().balance(&miner), BALANCE_CAP);
    assert_eq!(chain.state().totals().expired, BLOCK_REWARD);
    chain.state().audit(chain.height()).unwrap();
}

#[test]
fn tx_exp_rules() {
    let mut chain = new_chain();
    mine(&mut chain, key(1));
    let block = template(&chain, key(1));
    let (expire, auto, cb) = system(&block);

    let wrong_height = RecordCommitment {
        height: Height(99),
        ..expire
    };
    let wrong_count = RecordCommitment { count: 1, ..expire };
    let wrong_root = RecordCommitment {
        records_root: Hash([7; 32]),
        ..expire
    };
    let cases = [
        (wrong_height, "TX-EXP-01"),
        (wrong_count, "TX-EXP-02"),
        (wrong_root, "TX-EXP-02"),
    ];
    for (bad, expected) in cases {
        let b = with_txs(
            &block,
            vec![
                Tx::Expire(bad),
                Tx::AutoCancel(auto),
                Tx::Coinbase(cb.clone()),
            ],
        );
        assert_eq!(rule(&mut chain, &b), expected);
    }
}

#[test]
fn tx_ac_rules() {
    let mut chain = new_chain();
    let block = template(&chain, key(1));
    let (expire, auto, cb) = system(&block);
    let cases = [
        (
            RecordCommitment {
                height: Height(5),
                ..auto
            },
            "TX-AC-01",
        ),
        (RecordCommitment { count: 1, ..auto }, "TX-AC-02"),
    ];
    for (bad, expected) in cases {
        let b = with_txs(
            &block,
            vec![
                Tx::Expire(expire),
                Tx::AutoCancel(bad),
                Tx::Coinbase(cb.clone()),
            ],
        );
        assert_eq!(rule(&mut chain, &b), expected);
    }
}

#[test]
fn tx_cb_rules() {
    let mut chain = new_chain();
    let block = template(&chain, key(1));
    let (expire, auto, cb) = system(&block);
    let cases = [
        (
            Coinbase {
                height: Height(2),
                ..cb.clone()
            },
            "TX-CB-01",
        ),
        (
            Coinbase {
                beneficiary: PubKey([0; 32]),
                ..cb.clone()
            },
            "TX-CB-02",
        ),
        (
            Coinbase {
                beneficiary: PubKey([2; 32]),
                ..cb.clone()
            },
            "TX-CB-02",
        ),
    ];
    for (bad, expected) in cases {
        let b = with_txs(
            &block,
            vec![Tx::Expire(expire), Tx::AutoCancel(auto), Tx::Coinbase(bad)],
        );
        assert_eq!(rule(&mut chain, &b), expected);
    }
}

#[test]
fn blk_09_layout() {
    let mut chain = new_chain();
    let block = template(&chain, key(1));
    let (expire, auto, cb) = system(&block);
    let other_cb = Tx::Coinbase(Coinbase {
        extra: BoundedBytes::new(vec![1]).unwrap(),
        ..cb.clone()
    });
    let layouts = [
        vec![
            Tx::Coinbase(cb.clone()),
            Tx::Expire(expire),
            Tx::AutoCancel(auto),
        ],
        vec![Tx::Expire(expire), Tx::AutoCancel(auto)],
        vec![
            Tx::Expire(expire),
            Tx::AutoCancel(auto),
            Tx::Coinbase(cb.clone()),
            other_cb,
        ],
        vec![],
    ];
    for txs in layouts {
        assert_eq!(rule(&mut chain, &with_txs(&block, txs)), "BLK-09");
    }
}

#[test]
fn blk_10_and_blk_11() {
    let mut chain = new_chain();
    let block = solve(template(&chain, key(1)));
    let (expire, auto, cb) = system(&block);

    let mut wrong_root = block.clone();
    wrong_root.header.tx_root = Hash([3; 32]);
    assert_eq!(rule(&mut chain, &solve(wrong_root)), "BLK-10");

    let dup = with_txs(
        &block,
        vec![
            Tx::Expire(expire),
            Tx::Expire(expire),
            Tx::AutoCancel(auto),
            Tx::Coinbase(cb),
        ],
    );
    assert_eq!(rule(&mut chain, &dup), "BLK-11");

    assert!(chain.connect(&block).is_ok());
}

#[test]
fn invalid_block_leaves_chain_unchanged() {
    let mut chain = new_chain();
    mine(&mut chain, key(1));
    let before = chain.state().clone();
    let tip = *chain.tip();
    let mut bad = solve(template(&chain, key(1)));
    bad.header.nonce += 1;
    while meets_target(&bad.header) {
        bad.header.nonce += 1;
    }
    assert_eq!(
        chain.connect(&bad),
        Err(BlockError::Header(
            yuanlai_consensus::HeaderError::InsufficientWork
        ))
    );
    assert_eq!(chain.state(), &before);
    assert_eq!(*chain.tip(), tip);
}

#[test]
fn disconnect_restores_exact_state() {
    let mut chain = new_chain();
    let mut snapshots = vec![chain.state().clone()];
    for i in 0..60 {
        mine(&mut chain, key(u8::try_from(i % 3).unwrap() + 1));
        snapshots.push(chain.state().clone());
    }
    while chain.disconnect_tip().is_some() {
        snapshots.pop();
        assert_eq!(chain.state(), snapshots.last().unwrap());
    }
    assert_eq!(chain.height(), Height(0));
    assert_eq!(chain.state(), &ChainState::default());
}

#[test]
fn outpoint_order_is_numeric_on_index() {
    let a = OutPoint {
        txid: Hash([1; 32]),
        index: 1,
    };
    let b = OutPoint {
        txid: Hash([1; 32]),
        index: 256,
    };
    assert!(
        a < b,
        "index must compare numerically, not by little-endian bytes"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    /// Mines past the first expiry with random miners and spacing; after every block all
    /// state invariants hold (INV-01..04), and undoing then redoing a block is exact (INV-13).
    #[test]
    fn invariants_hold_across_expiry(
        miners in proptest::collection::vec(0_u8..4, 1_500),
        gaps in proptest::collection::vec(1_u64..200, 1_500),
    ) {
        let keys: Vec<PubKey> = (1..=4).map(key).collect();
        let mut chain = new_chain();
        for (i, (m, gap)) in miners.iter().zip(&gaps).enumerate() {
            let ts = Timestamp(chain.min_next_timestamp().0.max(chain.tip().timestamp.0 + gap));
            let block = solve(chain.template(keys[usize::from(*m)], BoundedBytes::empty(), ts).unwrap());
            chain.connect(&block).unwrap();
            if let Err(v) = chain.state().audit(chain.height()) {
                prop_assert!(false, "height {}: {:?}", chain.height(), v);
            }
            if i % 97 == 0 {
                let after = chain.state().clone();
                chain.disconnect_tip().unwrap();
                chain.connect(&block).unwrap();
                prop_assert_eq!(chain.state(), &after);
            }
        }
        prop_assert!(chain.state().totals().expired > Credit::ZERO);
    }
}
