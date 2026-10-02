//! Shared helpers for consensus integration tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test helpers: any failure should abort the test"
)]

use yuanlai_codec::{BoundedText, BoundedVec};
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{BlockError, Chain, meets_target};
use yuanlai_crypto::{public_key, sign};
use yuanlai_types::params::MAX_RENEWALS;
use yuanlai_types::{
    AddLock, AppendFiles, BeginCourse, Block, Credit, FileEntry, Hash, Height, Network, OutPoint,
    PubKey, RegisterIdentity, SessionRef, Signature, Timestamp, Tx, UserTx, Utxo,
};

pub const A: u8 = 1;
pub const B: u8 = 2;
pub const C: u8 = 3;
pub const FILLER: u8 = 9;

pub fn secret(seed: u8) -> [u8; 32] {
    [seed; 32]
}

pub fn pk(seed: u8) -> PubKey {
    PubKey(public_key(&secret(seed)))
}

pub fn commitment(n: u8) -> Hash {
    let mut bytes = [n; 32];
    bytes[31] = 0;
    Hash(bytes)
}

pub fn next_height(chain: &Chain) -> Height {
    Height(chain.height().0 + 1)
}

/// Mines a block; on success audits all invariants and checks that undo + redo is exact.
pub fn mine_to(chain: &mut Chain, beneficiary: PubKey, txs: Vec<Tx>) -> Result<Block, BlockError> {
    let ts = Timestamp(
        chain
            .min_next_timestamp()
            .0
            .max(chain.tip().timestamp.0 + 60),
    );
    let mut block = chain
        .template_with(beneficiary, yuanlai_codec::BoundedBytes::empty(), ts, txs)
        .unwrap();
    while !meets_target(&block.header) {
        block.header.nonce += 1;
    }
    let before = chain.state().clone();
    chain.connect(&block)?;
    if let Err(v) = chain.state().audit(chain.height()) {
        panic!("invariants violated at {}: {v:?}", chain.height());
    }
    let after = chain.state().clone();
    chain.disconnect_tip().unwrap();
    assert_eq!(chain.state(), &before, "undo is not exact");
    chain.connect(&block).unwrap();
    assert_eq!(chain.state(), &after, "redo is not exact");
    Ok(block)
}

pub fn mine(chain: &mut Chain, txs: Vec<Tx>) -> Result<Block, BlockError> {
    mine_to(chain, pk(FILLER), txs)
}

pub fn mine_blocks(chain: &mut Chain, beneficiary: u8, n: u64) {
    for _ in 0..n {
        mine_to(chain, pk(beneficiary), vec![]).unwrap();
    }
}

/// Mines empty blocks until the next block has height `h`. Audits every 64 blocks only.
pub fn mine_until_next(chain: &mut Chain, h: u64) {
    while next_height(chain).0 < h {
        let ts = Timestamp(
            chain
                .min_next_timestamp()
                .0
                .max(chain.tip().timestamp.0 + 60),
        );
        let mut block = chain
            .template(pk(FILLER), yuanlai_codec::BoundedBytes::empty(), ts)
            .unwrap();
        while !meets_target(&block.header) {
            block.header.nonce += 1;
        }
        chain.connect(&block).unwrap();
        if chain.height().0.is_multiple_of(64) {
            chain.state().audit(chain.height()).unwrap();
        }
    }
    chain.state().audit(chain.height()).unwrap();
    assert_eq!(next_height(chain).0, h);
}

pub fn assert_rejected(chain: &mut Chain, txs: Vec<Tx>, rule: &str) {
    let before = chain.state().clone();
    let tip = chain.tip().id();
    let err = mine(chain, txs).unwrap_err();
    assert_eq!(err.rule_id(), rule, "{err}");
    assert_eq!(chain.state(), &before);
    assert_eq!(chain.tip().id(), tip);
}

/// Builds the transactions before borrowing the chain mutably, then expects rejection.
macro_rules! reject {
    ($chain:expr, $txs:expr, $rule:expr) => {{
        let txs = $txs;
        $crate::common::assert_rejected(&mut $chain, txs, $rule);
    }};
}

/// Builds the transactions before borrowing the chain mutably, then mines them.
macro_rules! accept {
    ($chain:expr, $txs:expr) => {{
        let txs = $txs;
        $crate::common::mine(&mut $chain, txs).unwrap()
    }};
}

pub fn signed(chain: &Chain, seed: u8, mut tx: Tx) -> Tx {
    let message = tx.sig_msg(&chain.params().chain_id).unwrap();
    tx.set_signature(Signature(sign(&secret(seed), message.as_bytes())));
    tx
}

pub fn user<B>(seed: u8, nonce: u64, body: B) -> UserTx<B> {
    UserTx {
        sender: pk(seed),
        nonce,
        body,
        signature: Signature::EMPTY,
    }
}

pub fn inputs_for(
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

pub fn nonce(chain: &Chain, seed: u8) -> u64 {
    chain.state().nonce(&pk(seed))
}

pub fn register(chain: &Chain, seed: u8, c: Hash) -> Tx {
    let inputs = BoundedVec::new(inputs_for(chain, seed, 20, |_| true)).unwrap();
    let body = RegisterIdentity {
        inputs,
        commitment: c,
    };
    signed(
        chain,
        seed,
        Tx::RegisterIdentity(user(seed, nonce(chain, seed), body)),
    )
}

pub fn new_chain() -> Chain {
    let network = Network::Regtest;
    Chain::new(network.params(), &genesis_block(network).unwrap()).unwrap()
}

/// A and B each mine 40 blocks and register identities (380 credit each).
pub fn world() -> Chain {
    let mut chain = new_chain();
    mine_blocks(&mut chain, A, 40);
    mine_blocks(&mut chain, B, 40);
    accept!(
        chain,
        vec![
            register(&chain, A, commitment(1)),
            register(&chain, B, commitment(2))
        ]
    );
    chain
}

pub fn text<const N: u32>(s: &str) -> BoundedText<N> {
    BoundedText::new(s.to_owned()).unwrap()
}

pub fn entry(path: &str, content: u8, size: u64) -> FileEntry {
    FileEntry {
        path: text(path),
        content_hash: Hash([content; 32]),
        size,
    }
}

pub fn begin_with(
    chain: &Chain,
    seed: u8,
    nonce: u64,
    inputs: Vec<OutPoint>,
    parent: Option<Hash>,
    declared: (u32, u64),
    lock: u64,
) -> Tx {
    let body = BeginCourse {
        inputs: BoundedVec::new(inputs).unwrap(),
        parent_root: parent,
        title: text("课程"),
        version: text("1.0"),
        declared_files: declared.0,
        declared_bytes: declared.1,
        lock: Credit(lock),
    };
    signed(chain, seed, Tx::BeginCourse(user(seed, nonce, body)))
}

pub fn begin(chain: &Chain, seed: u8, parent: Option<Hash>, declared: (u32, u64), lock: u64) -> Tx {
    let inputs = inputs_for(chain, seed, lock, |u| u.renewals < MAX_RENEWALS);
    begin_with(
        chain,
        seed,
        nonce(chain, seed),
        inputs,
        parent,
        declared,
        lock,
    )
}

pub fn append(chain: &Chain, seed: u8, nonce: u64, session: Hash, entries: Vec<FileEntry>) -> Tx {
    let body = AppendFiles {
        session_id: session,
        entries: BoundedVec::new(entries).unwrap(),
    };
    signed(chain, seed, Tx::AppendFiles(user(seed, nonce, body)))
}

pub fn add_lock(
    chain: &Chain,
    seed: u8,
    nonce: u64,
    session: Hash,
    declared: (u32, u64),
    amount: u64,
) -> Tx {
    let inputs = if amount == 0 {
        Vec::new()
    } else {
        inputs_for(chain, seed, amount, |u| u.renewals < MAX_RENEWALS)
    };
    let body = AddLock {
        inputs: BoundedVec::new(inputs).unwrap(),
        session_id: session,
        declared_files: declared.0,
        declared_bytes: declared.1,
        amount: Credit(amount),
    };
    signed(chain, seed, Tx::AddLock(user(seed, nonce, body)))
}

pub fn finalize(chain: &Chain, seed: u8, nonce: u64, session: Hash) -> Tx {
    let body = SessionRef {
        session_id: session,
    };
    signed(chain, seed, Tx::FinalizeCourse(user(seed, nonce, body)))
}

pub fn cancel(chain: &Chain, seed: u8, nonce: u64, session: Hash) -> Tx {
    let body = SessionRef {
        session_id: session,
    };
    signed(chain, seed, Tx::CancelCourse(user(seed, nonce, body)))
}
