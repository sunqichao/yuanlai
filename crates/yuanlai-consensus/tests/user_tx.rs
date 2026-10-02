//! Tests for user transactions: `spec/03-transactions.md` §2–§6 and `spec/04-credit.md` §4.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test code: any failure should abort the test"
)]

use proptest::prelude::*;
use yuanlai_codec::{BoundedBytes, BoundedVec};
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{BlockError, Chain, course_cost, course_root, meets_target, transfer_loss};
use yuanlai_crypto::{public_key, sha256, sign};
use yuanlai_types::params::{
    CIRCUIT_FIELD_MODULUS, COST_VERIFY_AUTHOR, EXPIRY_BLOCKS, IDENTITY_CIRCUIT_K, MAX_HOPS,
    MAX_USER_TXS_PER_SENDER_PER_BLOCK, NO_RETURN_WINDOW,
};
use yuanlai_types::{
    AppendFiles, BeginCourse, Block, CoursePath, CourseTitle, CourseVersion, Credit, FileEntry,
    Hash, Height, Network, OutPoint, PubKey, RegisterIdentity, SessionRef, Signature, Source,
    Timestamp, Transfer, Tx, UserTx, Utxo, VerifyAuthor,
};
use yuanlai_zk::{identity_commitment, prove_author, setup, verify_author_consensus};

const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 3;
const D: u8 = 4;
const FILLER: u8 = 9;

/// Builds the transactions before borrowing the chain mutably, then expects rejection.
macro_rules! reject {
    ($chain:expr, $txs:expr, $rule:expr) => {{
        let txs = $txs;
        assert_rejected(&mut $chain, txs, $rule);
    }};
}

/// Builds the transactions before borrowing the chain mutably, then mines them.
macro_rules! accept {
    ($chain:expr, $txs:expr) => {{
        let txs = $txs;
        mine(&mut $chain, txs).unwrap()
    }};
}

fn secret(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn pk(seed: u8) -> PubKey {
    PubKey(public_key(&secret(seed)))
}

fn commitment(n: u8) -> Hash {
    let mut bytes = [n; 32];
    bytes[31] = 0;
    Hash(bytes)
}

fn next_height(chain: &Chain) -> Height {
    Height(chain.height().0 + 1)
}

fn mine_to(chain: &mut Chain, beneficiary: PubKey, txs: Vec<Tx>) -> Result<Block, BlockError> {
    let ts = Timestamp(
        chain
            .min_next_timestamp()
            .0
            .max(chain.tip().timestamp.0 + 60),
    );
    let mut block = chain
        .template_with(beneficiary, BoundedBytes::empty(), ts, txs)
        .unwrap();
    while !meets_target(&block.header) {
        block.header.nonce += 1;
    }
    chain.connect(&block).map(|()| block)
}

fn mine(chain: &mut Chain, txs: Vec<Tx>) -> Result<Block, BlockError> {
    mine_to(chain, pk(FILLER), txs)
}

fn mine_blocks(chain: &mut Chain, beneficiary: u8, n: usize) {
    for _ in 0..n {
        mine_to(chain, pk(beneficiary), vec![]).unwrap();
    }
}

/// Asserts that a block with `txs` is rejected for `rule` and that the chain is unchanged.
fn assert_rejected(chain: &mut Chain, txs: Vec<Tx>, rule: &str) {
    let before = chain.state().clone();
    let tip = chain.tip().id();
    let err = mine(chain, txs).unwrap_err();
    assert_eq!(err.rule_id(), rule, "{err}");
    assert_eq!(chain.state(), &before);
    assert_eq!(chain.tip().id(), tip);
}

fn signed(chain: &Chain, seed: u8, mut tx: Tx) -> Tx {
    let message = tx.sig_msg(&chain.params().chain_id).unwrap();
    tx.set_signature(Signature(sign(&secret(seed), message.as_bytes())));
    tx
}

fn transfer_tx(
    chain: &Chain,
    seed: u8,
    nonce: u64,
    inputs: Vec<OutPoint>,
    to: PubKey,
    amount: u64,
) -> Tx {
    let tx = Tx::Transfer(UserTx {
        sender: pk(seed),
        nonce,
        body: Transfer {
            inputs: BoundedVec::new(inputs).unwrap(),
            recipient: to,
            amount: Credit(amount),
        },
        signature: Signature::EMPTY,
    });
    signed(chain, seed, tx)
}

fn inputs_for(
    chain: &Chain,
    seed: u8,
    amount: u64,
    filter: impl Fn(&Utxo) -> bool,
) -> Vec<OutPoint> {
    chain
        .state()
        .select_inputs(&pk(seed), Credit(amount), next_height(chain), filter)
        .unwrap()
}

fn transfer(chain: &Chain, seed: u8, to: u8, amount: u64) -> Tx {
    let inputs = inputs_for(chain, seed, amount, |u| u.hops < MAX_HOPS);
    let nonce = chain.state().nonce(&pk(seed));
    transfer_tx(chain, seed, nonce, inputs, pk(to), amount)
}

fn register_tx(chain: &Chain, seed: u8, nonce: u64, inputs: Vec<OutPoint>, c: Hash) -> Tx {
    let tx = Tx::RegisterIdentity(UserTx {
        sender: pk(seed),
        nonce,
        body: RegisterIdentity {
            inputs: BoundedVec::new(inputs).unwrap(),
            commitment: c,
        },
        signature: Signature::EMPTY,
    });
    signed(chain, seed, tx)
}

fn register(chain: &Chain, seed: u8, c: Hash) -> Tx {
    let inputs = inputs_for(chain, seed, 20, |_| true);
    register_tx(chain, seed, chain.state().nonce(&pk(seed)), inputs, c)
}

fn register_one_file_course(chain: &Chain, seed: u8) -> (Hash, Vec<Tx>) {
    let owner = pk(seed);
    let entry = FileEntry {
        path: CoursePath::new("lesson.md".to_owned()).unwrap(),
        content_hash: sha256(b"author verification course"),
        size: 27,
    };
    let root = course_root([&entry]);
    let lock = course_cost(false, 1, entry.size).unwrap();
    let nonce = chain.state().nonce(&owner);
    let begin = signed(
        chain,
        seed,
        Tx::BeginCourse(UserTx {
            sender: owner,
            nonce,
            body: BeginCourse {
                inputs: BoundedVec::new(inputs_for(chain, seed, lock.0, |_| true)).unwrap(),
                parent_root: None,
                title: CourseTitle::new("Author proof test".to_owned()).unwrap(),
                version: CourseVersion::new("1".to_owned()).unwrap(),
                declared_files: 1,
                declared_bytes: entry.size,
                lock,
            },
            signature: Signature::EMPTY,
        }),
    );
    let session_id = begin.txid();
    let append = signed(
        chain,
        seed,
        Tx::AppendFiles(UserTx {
            sender: owner,
            nonce: nonce + 1,
            body: AppendFiles {
                session_id,
                entries: BoundedVec::new(vec![entry]).unwrap(),
            },
            signature: Signature::EMPTY,
        }),
    );
    let finalize = signed(
        chain,
        seed,
        Tx::FinalizeCourse(UserTx {
            sender: owner,
            nonce: nonce + 2,
            body: SessionRef { session_id },
            signature: Signature::EMPTY,
        }),
    );
    (root, vec![begin, append, finalize])
}

fn verify_author_tx(
    chain: &Chain,
    seed: u8,
    course_root: Hash,
    challenge: Hash,
    nullifier: Hash,
    proof: Vec<u8>,
) -> Tx {
    let owner = pk(seed);
    signed(
        chain,
        seed,
        Tx::VerifyAuthor(UserTx {
            sender: owner,
            nonce: chain.state().nonce(&owner),
            body: VerifyAuthor {
                inputs: BoundedVec::new(inputs_for(chain, seed, COST_VERIFY_AUTHOR.0, |_| true))
                    .unwrap(),
                course_root,
                challenge,
                nullifier,
                proof: BoundedBytes::new(proof).unwrap(),
            },
            signature: Signature::EMPTY,
        }),
    )
}

/// A mines 40 blocks and B 15; both then register identities (A: 380, B: 130).
fn world() -> Chain {
    let network = Network::Regtest;
    let mut chain = Chain::new(network.params(), &genesis_block(network).unwrap()).unwrap();
    mine_blocks(&mut chain, A, 40);
    mine_blocks(&mut chain, B, 15);
    accept!(
        chain,
        vec![
            register(&chain, A, commitment(1)),
            register(&chain, B, commitment(2))
        ]
    );
    chain
}

fn outputs_of(chain: &Chain, tx: &Tx) -> Vec<(u32, Utxo)> {
    let txid = tx.txid();
    chain
        .state()
        .utxos()
        .filter(|(o, _)| o.txid == txid)
        .map(|(o, u)| (o.index, *u))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Identity

#[test]
fn register_identity_effects() {
    let chain = world();
    let s = chain.state();
    assert_eq!(s.identity(&pk(A)), Some(commitment(1)));
    assert_eq!(s.identity_owner(&commitment(2)), Some(pk(B)));
    assert_eq!(s.nonce(&pk(A)), 1);
    assert_eq!(s.balance(&pk(A)), Credit(380));
    assert_eq!(s.balance(&pk(B)), Credit(130));
    assert_eq!(s.totals().burned, Credit(40));
    s.audit(chain.height()).unwrap();
}

#[test]
fn verify_author_checks_proof_nullifier_and_reorg_undo() {
    let secret_bytes = secret(A);
    let salt = secret(0x2a);
    let identity = identity_commitment(&secret_bytes, &salt).unwrap();
    let mut chain = Chain::new(
        Network::Regtest.params(),
        &genesis_block(Network::Regtest).unwrap(),
    )
    .unwrap();
    mine_blocks(&mut chain, A, 40);
    accept!(chain, vec![register(&chain, A, identity)]);

    let (root, course_txs) = register_one_file_course(&chain, A);
    accept!(chain, course_txs);

    let challenge = Hash([0x37; 32]);
    let proving_parameters = setup(IDENTITY_CIRCUIT_K).unwrap();
    let author_proof = prove_author(
        &proving_parameters,
        &secret_bytes,
        &salt,
        &challenge,
        &root,
        rand::rand_core::UnwrapErr(rand::rngs::SysRng),
    )
    .unwrap();
    verify_author_consensus(
        &identity,
        &challenge,
        &root,
        &author_proof.nullifier,
        &author_proof.proof,
    )
    .unwrap();

    let mut corrupted_proof = author_proof.proof.clone();
    corrupted_proof[0] ^= 1;
    let invalid = verify_author_tx(
        &chain,
        A,
        root,
        challenge,
        author_proof.nullifier,
        corrupted_proof,
    );
    reject!(chain, vec![invalid], "TX-VA-04");

    let unowned_course = verify_author_tx(
        &chain,
        A,
        Hash([0x99; 32]),
        challenge,
        author_proof.nullifier,
        author_proof.proof.clone(),
    );
    reject!(chain, vec![unowned_course], "TX-VA-01");

    let verification_tx = verify_author_tx(
        &chain,
        A,
        root,
        challenge,
        author_proof.nullifier,
        author_proof.proof.clone(),
    );
    let burned_before = chain.state().totals().burned;
    let verification_block = accept!(chain, vec![verification_tx]);
    let record = chain
        .state()
        .author_verification(&author_proof.nullifier)
        .unwrap();
    assert_eq!(record.sender, pk(A));
    assert_eq!(record.course_root, root);
    assert_eq!(record.challenge, challenge);
    assert_eq!(chain.state().author_verifications().count(), 1);
    assert_eq!(
        chain.state().totals().burned,
        Credit(burned_before.0 + COST_VERIFY_AUTHOR.0)
    );

    let replay = verify_author_tx(
        &chain,
        A,
        root,
        challenge,
        author_proof.nullifier,
        author_proof.proof,
    );
    reject!(chain, vec![replay], "TX-VA-03");

    let (_, delta) = chain.disconnect_tip().unwrap();
    assert_eq!(delta.author_verifications.len(), 1);
    assert!(
        chain
            .state()
            .author_verification(&delta.author_verifications[0].nullifier)
            .is_none()
    );
    chain.connect(&verification_block).unwrap();
    assert!(
        chain
            .state()
            .author_verification(&author_proof.nullifier)
            .is_some()
    );
}

#[test]
fn tx_id_01_to_03() {
    let mut chain = world();
    reject!(chain, vec![register(&chain, A, commitment(7))], "TX-ID-01");

    mine_blocks(&mut chain, C, 12);
    reject!(chain, vec![register(&chain, C, commitment(1))], "TX-ID-02");

    let mut p_le = CIRCUIT_FIELD_MODULUS.to_be_bytes();
    p_le.reverse();
    reject!(chain, vec![register(&chain, C, Hash(p_le))], "TX-ID-03");
    reject!(
        chain,
        vec![register(&chain, C, Hash([0xFF; 32]))],
        "TX-ID-03"
    );

    // p − 1 is the largest canonical element.
    p_le[0] -= 1;
    accept!(chain, vec![register(&chain, C, Hash(p_le))]);
}

#[test]
fn same_commitment_twice_in_one_block_is_rejected() {
    let mut chain = world();
    mine_blocks(&mut chain, C, 12);
    mine_blocks(&mut chain, D, 12);
    reject!(
        chain,
        vec![
            register(&chain, C, commitment(5)),
            register(&chain, D, commitment(5))
        ],
        "TX-ID-02"
    );
}

// ---------------------------------------------------------------------------------------------
// Signatures and nonces

#[test]
fn tx_sig_01() {
    let mut chain = world();
    let good = transfer(&chain, A, B, 25);

    let mut flipped = good.clone();
    let Tx::Transfer(t) = &mut flipped else {
        panic!()
    };
    t.signature.0[0] ^= 1;
    reject!(chain, vec![flipped], "TX-SIG-01");

    let mut tampered = good.clone();
    let Tx::Transfer(t) = &mut tampered else {
        panic!()
    };
    t.body.amount = Credit(24);
    reject!(chain, vec![tampered], "TX-SIG-01");

    // Signed for another network: chain_id is part of the signed message.
    let mut other_chain = good.clone();
    let msg = other_chain.sig_msg(&Network::Testnet.chain_id()).unwrap();
    other_chain.set_signature(Signature(sign(&secret(A), msg.as_bytes())));
    reject!(chain, vec![other_chain], "TX-SIG-01");

    let mut weak_sender = good.clone();
    let Tx::Transfer(t) = &mut weak_sender else {
        panic!()
    };
    t.sender = PubKey([0; 32]);
    reject!(chain, vec![weak_sender], "TX-SIG-01");

    accept!(chain, vec![good]);
}

#[test]
fn tx_nonce_01_sequencing_and_replay() {
    let mut chain = world();
    let n = chain.state().nonce(&pk(A));
    let inputs = inputs_for(&chain, A, 30, |_| true);
    let (first, second) = inputs.split_at(2);

    reject!(
        chain,
        vec![transfer_tx(&chain, A, n + 1, first.to_vec(), pk(B), 20)],
        "TX-NONCE-01"
    );

    let t1 = transfer_tx(&chain, A, n, first.to_vec(), pk(B), 20);
    let t2 = transfer_tx(&chain, A, n + 1, second.to_vec(), pk(B), 10);
    accept!(chain, vec![t1.clone(), t2]);
    assert_eq!(chain.state().nonce(&pk(A)), n + 2);

    // Replaying a confirmed transaction fails on its nonce before anything else.
    reject!(chain, vec![t1], "TX-NONCE-01");
}

// ---------------------------------------------------------------------------------------------
// Inputs

#[test]
fn tx_in_rules() {
    let mut chain = world();
    let n = chain.state().nonce(&pk(A));
    let ins = inputs_for(&chain, A, 30, |_| true);
    let [i0, i1, i2] = ins[..] else { panic!() };
    let unknown = OutPoint {
        txid: Hash([5; 32]),
        index: 0,
    };

    let cases: Vec<(Vec<OutPoint>, u64, &str)> = vec![
        (vec![i0, i0], 15, "TX-IN-01"),
        (vec![unknown], 5, "TX-IN-02"),
        (vec![i1, i0], 15, "TX-IN-03"),
        (vec![i0, i1, i2], 15, "TX-IN-04"),
        (vec![i0], 15, "TX-IN-04"),
        (vec![], 5, "TX-IN-05"),
    ];
    for (inputs, amount, rule) in cases {
        reject!(
            chain,
            vec![transfer_tx(&chain, A, n, inputs, pk(B), amount)],
            rule
        );
    }

    let b_utxo = inputs_for(&chain, B, 5, |_| true);
    reject!(
        chain,
        vec![transfer_tx(&chain, A, n, b_utxo, pk(B), 5)],
        "TX-IN-02"
    );

    // A fresh reward is immature.
    let fresh = mine_to(&mut chain, pk(A), vec![]).unwrap();
    let immature = OutPoint {
        txid: fresh.txs.as_slice()[2].txid(),
        index: 0,
    };
    reject!(
        chain,
        vec![transfer_tx(&chain, A, n, vec![immature], pk(B), 5)],
        "TX-IN-02"
    );
}

#[test]
fn inputs_spent_earlier_in_the_block_are_missing() {
    let mut chain = world();
    let n = chain.state().nonce(&pk(A));
    let ins = inputs_for(&chain, A, 10, |_| true);
    reject!(
        chain,
        vec![
            transfer_tx(&chain, A, n, ins.clone(), pk(B), 10),
            transfer_tx(&chain, A, n + 1, ins, pk(B), 10),
        ],
        "TX-IN-02"
    );
}

// ---------------------------------------------------------------------------------------------
// Transfer

#[test]
fn transfer_effects() {
    let mut chain = world();
    let h = next_height(&chain);
    let ins = inputs_for(&chain, A, 25, |_| true);
    assert_eq!(ins.len(), 3);
    let last_input = *chain.state().utxo(&ins[2]).unwrap();
    let burned_before = chain.state().totals().burned;

    let tx = transfer(&chain, A, B, 25);
    accept!(chain, vec![tx.clone()]);

    let outs = outputs_of(&chain, &tx);
    let (_, received) = outs.iter().find(|(i, _)| *i == 0).unwrap();
    assert_eq!(received.owner, pk(B));
    assert_eq!(received.amount, Credit(23));
    assert_eq!(received.source, Source::Received);
    assert_eq!(received.acquired_height, h);
    assert_eq!(received.spendable_from, h);
    assert_eq!(received.expire_height, Height(h.0 + EXPIRY_BLOCKS));
    assert_eq!(received.hops, 1);

    // Change keeps the last input's attributes, including its original expiry.
    let (_, change) = outs.iter().find(|(i, _)| *i == 1).unwrap();
    assert_eq!(
        *change,
        Utxo {
            amount: Credit(5),
            ..last_input
        }
    );

    assert_eq!(chain.state().totals().burned, Credit(burned_before.0 + 2));
    assert_eq!(chain.state().balance(&pk(A)), Credit(355));
    assert_eq!(chain.state().balance(&pk(B)), Credit(153));
    chain.state().audit(chain.height()).unwrap();
}

#[test]
fn loss_is_ten_percent_with_minimum_one() {
    let cases = [
        (2, 1),
        (9, 1),
        (10, 1),
        (19, 1),
        (20, 2),
        (25, 2),
        (100, 10),
        (199, 19),
    ];
    for (amount, loss) in cases {
        assert_eq!(
            transfer_loss(Credit(amount)),
            Some(Credit(loss)),
            "{amount}"
        );
    }
}

#[test]
fn tx_transfer_01_02_04() {
    let mut chain = world();
    reject!(chain, vec![transfer(&chain, A, B, 1)], "TX-TRANSFER-01");
    reject!(chain, vec![transfer(&chain, A, A, 5)], "TX-TRANSFER-02");
    reject!(chain, vec![transfer(&chain, A, C, 5)], "TX-TRANSFER-04");
}

#[test]
fn recipient_may_register_earlier_in_the_same_block() {
    let mut chain = world();
    mine_blocks(&mut chain, C, 12);
    accept!(
        chain,
        vec![
            register(&chain, C, commitment(3)),
            transfer(&chain, A, C, 10)
        ]
    );
}

#[test]
fn tx_transfer_03_hops_are_exhausted_after_three_transfers() {
    let mut chain = world();
    mine_blocks(&mut chain, C, 12);
    mine_blocks(&mut chain, D, 12);
    accept!(
        chain,
        vec![
            register(&chain, C, commitment(3)),
            register(&chain, D, commitment(4))
        ]
    );

    let hop = |chain: &Chain, from: u8, to: u8, hops: u8, amount: u64| {
        let ins = inputs_for(chain, from, amount, |u| u.hops == hops);
        transfer_tx(
            chain,
            from,
            chain.state().nonce(&pk(from)),
            ins,
            pk(to),
            amount,
        )
    };
    accept!(chain, vec![hop(&chain, A, B, 0, 30)]);
    accept!(chain, vec![hop(&chain, B, C, 1, 20)]);
    accept!(chain, vec![hop(&chain, C, D, 2, 15)]);

    let exhausted = chain
        .state()
        .utxos()
        .find(|(_, u)| u.owner == pk(D) && u.hops == MAX_HOPS)
        .map(|(o, _)| *o)
        .unwrap();
    let n = chain.state().nonce(&pk(D));
    reject!(
        chain,
        vec![transfer_tx(&chain, D, n, vec![exhausted], pk(A), 5)],
        "TX-TRANSFER-03"
    );
}

#[test]
fn tx_transfer_05_no_return_until_window_passes() {
    let mut chain = world();
    accept!(chain, vec![transfer(&chain, A, B, 20)]);
    let sent_at = chain.height();

    reject!(chain, vec![transfer(&chain, B, A, 5)], "TX-TRANSFER-05");

    // The window is (h − W, h]: blocked through sent_at + W − 1, allowed from sent_at + W.
    while next_height(&chain).0 < sent_at.0 + NO_RETURN_WINDOW - 1 {
        mine_to(&mut chain, pk(B), vec![]).unwrap();
    }
    assert_eq!(next_height(&chain).0, sent_at.0 + NO_RETURN_WINDOW - 1);
    reject!(chain, vec![transfer(&chain, B, A, 5)], "TX-TRANSFER-05");
    mine_to(&mut chain, pk(B), vec![]).unwrap();
    assert_eq!(next_height(&chain).0, sent_at.0 + NO_RETURN_WINDOW);
    accept!(chain, vec![transfer(&chain, B, A, 5)]);
}

#[test]
fn tx_transfer_05_applies_within_one_block() {
    let mut chain = world();
    reject!(
        chain,
        vec![transfer(&chain, A, B, 20), transfer(&chain, B, A, 5)],
        "TX-TRANSFER-05"
    );
}

#[test]
fn tx_transfer_06_recipient_balance_cap() {
    let mut chain = world();
    mine_blocks(&mut chain, A, 12);
    assert_eq!(chain.state().balance(&pk(A)), Credit(500));
    reject!(chain, vec![transfer(&chain, B, A, 2)], "TX-TRANSFER-06");
}

#[test]
fn tx_transfer_07_inbound_cap_across_and_within_blocks() {
    let mut chain = world();
    accept!(chain, vec![transfer(&chain, A, B, 150)]);
    reject!(chain, vec![transfer(&chain, A, B, 80)], "TX-TRANSFER-07");

    let mut fresh = world();
    let n = fresh.state().nonce(&pk(A));
    let ins = inputs_for(&fresh, A, 230, |_| true);
    let (first, second) = ins.split_at(15);
    reject!(
        fresh,
        vec![
            transfer_tx(&fresh, A, n, first.to_vec(), pk(B), 150),
            transfer_tx(&fresh, A, n + 1, second.to_vec(), pk(B), 80),
        ],
        "TX-TRANSFER-07"
    );
}

// ---------------------------------------------------------------------------------------------
// Block-level rules

#[test]
fn blk_12_per_sender_limit() {
    let mut chain = world();
    let n = chain.state().nonce(&pk(A));
    let ins = inputs_for(&chain, A, 170, |_| true);
    let limit = usize::try_from(MAX_USER_TXS_PER_SENDER_PER_BLOCK).unwrap();
    let txs: Vec<Tx> = (0..=limit)
        .map(|i| {
            transfer_tx(
                &chain,
                A,
                n + u64::try_from(i).unwrap(),
                vec![ins[i]],
                pk(B),
                10,
            )
        })
        .collect();
    reject!(chain, txs.clone(), "BLK-12");
    accept!(chain, txs[..limit].to_vec());
}

#[test]
fn blk_01_block_size() {
    let mut chain = world();
    let fake: Vec<OutPoint> = (0..64)
        .map(|i| OutPoint {
            txid: Hash([7; 32]),
            index: i,
        })
        .collect();
    let txs: Vec<Tx> = (0..450)
        .map(|n| transfer_tx(&chain, A, n, fake.clone(), pk(B), 5))
        .collect();
    reject!(chain, txs, "BLK-01");
}

#[test]
fn blk_09_system_tx_after_user_txs() {
    let mut chain = world();
    // A Coinbase differing from the block's own (other beneficiary), so BLK-11 does not apply.
    let template = chain
        .template(pk(A), BoundedBytes::empty(), Timestamp(0))
        .unwrap();
    let coinbase = template.txs.as_slice()[2].clone();
    reject!(chain, vec![transfer(&chain, A, B, 10), coinbase], "BLK-09");
}

// ---------------------------------------------------------------------------------------------
// Invariants under random activity

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4))]

    #[test]
    fn invariants_hold_with_transfers(
        ops in proptest::collection::vec((0_u8..3, 0_u8..3, 2_u64..80, 0_u8..4), 400),
    ) {
        let parties = [A, B, C];
        let mut chain = world();
        mine_blocks(&mut chain, C, 12);
        accept!(chain, vec![register(&chain, C, commitment(3))]);

        for (i, (from, to, amount, miner)) in ops.into_iter().enumerate() {
            let (from, to) = (parties[usize::from(from)], parties[usize::from(to)]);
            let beneficiary = pk([A, B, C, FILLER][usize::from(miner)]);
            let candidate = chain
                .state()
                .select_inputs(&pk(from), Credit(amount), next_height(&chain), |u| u.hops < MAX_HOPS)
                .map(|ins| {
                    let n = chain.state().nonce(&pk(from));
                    transfer_tx(&chain, from, n, ins, pk(to), amount)
                });
            let accepted = match candidate {
                Some(tx) => match mine_to(&mut chain, beneficiary, vec![tx]) {
                    Ok(block) => Some(block),
                    Err(e) => {
                        let expected = ["TX-TRANSFER-02", "TX-TRANSFER-05", "TX-TRANSFER-06", "TX-TRANSFER-07"];
                        prop_assert!(expected.contains(&e.rule_id()), "unexpected {e}");
                        None
                    }
                },
                None => None,
            };
            let block = match accepted {
                Some(block) => block,
                None => mine_to(&mut chain, beneficiary, vec![]).unwrap(),
            };
            if let Err(v) = chain.state().audit(chain.height()) {
                prop_assert!(false, "height {}: {:?}", chain.height(), v);
            }
            if i % 37 == 0 {
                let after = chain.state().clone();
                chain.disconnect_tip().unwrap();
                chain.connect(&block).unwrap();
                prop_assert_eq!(chain.state(), &after);
            }
        }
        prop_assert!(chain.state().totals().burned > Credit(60));
    }
}
