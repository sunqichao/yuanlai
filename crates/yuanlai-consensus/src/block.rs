use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use yuanlai_codec::{BoundedBytes, BoundedVec, Encode};
use yuanlai_crypto::{is_valid_public_key, merkle_root};
use yuanlai_types::params::{
    BALANCE_CAP, BLOCK_REWARD, COINBASE_MATURITY, EXPIRY_BLOCKS, MAX_BLOCK_SIZE,
    MAX_COINBASE_EXTRA, MAX_USER_TXS_PER_SENDER_PER_BLOCK,
};
use yuanlai_types::{
    AutoCancelRecord, Block, BlockHeader, ChainParams, Coinbase, Credit, ExpireRecord, Hash,
    Height, OutPoint, PubKey, RecordCommitment, Source, Tx, Utxo,
};

use crate::course::{Session, cancel_fee};
use crate::course_tx::end_session;
use crate::delta::{BlockDelta, Overflow, Overlay};
use crate::state::ChainState;
use crate::user_tx::{TxError, apply_user_tx};
use crate::{Ancestry, HeaderError, validate_genesis_header, validate_header};

/// A block violates a rule of `spec/02-block.md` – `spec/04-credit.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// A header rule (BLK-02 – BLK-08).
    Header(HeaderError),
    /// BLK-01
    TooLarge,
    /// BLK-09
    BadLayout,
    /// BLK-10
    TxRootMismatch,
    /// BLK-11
    DuplicateTx,
    /// BLK-12
    TooManyFromSender(PubKey),
    /// A user transaction at `index` in the block is invalid.
    Tx {
        /// Position in `block.txs`.
        index: usize,
        /// The violated rule.
        error: TxError,
    },
    /// TX-CB-01
    CoinbaseHeight,
    /// TX-CB-02
    InvalidBeneficiary,
    /// TX-EXP-01
    ExpireHeight,
    /// TX-EXP-02
    ExpireMismatch,
    /// TX-AC-01
    AutoCancelHeight,
    /// TX-AC-02
    AutoCancelMismatch,
    /// Genesis body is not exactly one zero-reward Coinbase (`spec/02-block.md` §6).
    InvalidGenesisBody,
    /// A credit or height computation overflowed.
    Overflow,
}

impl BlockError {
    /// The spec rule identifier this error corresponds to.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::Header(e) => e.rule_id(),
            Self::TooLarge => "BLK-01",
            Self::BadLayout => "BLK-09",
            Self::TxRootMismatch => "BLK-10",
            Self::DuplicateTx => "BLK-11",
            Self::TooManyFromSender(_) => "BLK-12",
            Self::Tx { error, .. } => error.rule_id(),
            Self::CoinbaseHeight => "TX-CB-01",
            Self::InvalidBeneficiary => "TX-CB-02",
            Self::ExpireHeight => "TX-EXP-01",
            Self::ExpireMismatch => "TX-EXP-02",
            Self::AutoCancelHeight => "TX-AC-01",
            Self::AutoCancelMismatch => "TX-AC-02",
            Self::InvalidGenesisBody => "GENESIS",
            Self::Overflow => "OVERFLOW",
        }
    }
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(e) => e.fmt(f),
            other => write!(f, "{}: {other:?}", other.rule_id()),
        }
    }
}

impl core::error::Error for BlockError {}

impl From<HeaderError> for BlockError {
    fn from(e: HeaderError) -> Self {
        Self::Header(e)
    }
}

impl From<Overflow> for BlockError {
    fn from(_: Overflow) -> Self {
        Self::Overflow
    }
}

/// The expected `Expire` commitment and its records for `height` (`spec/04-credit.md` §6).
#[must_use]
pub fn expire_commitment(
    state: &ChainState,
    height: Height,
) -> (RecordCommitment, Vec<ExpireRecord>) {
    let records = state.expiring_at(height);
    let encoded: Vec<Vec<u8>> = records.iter().map(Encode::encode).collect();
    let commitment = RecordCommitment {
        height,
        count: u32::try_from(records.len()).unwrap_or(u32::MAX),
        records_root: merkle_root(&encoded),
    };
    (commitment, records)
}

/// Sessions whose deadline is `height`, in `session_id` order, with their records, and the
/// resulting `AutoCancel` commitment (`spec/05-course-session.md` §7).
#[must_use]
pub fn auto_cancel_commitment(
    state: &ChainState,
    height: Height,
) -> (RecordCommitment, Vec<(Session, AutoCancelRecord)>) {
    let courses = state.courses();
    let ending: Vec<(Session, AutoCancelRecord)> = courses
        .deadlines
        .range((height, Hash::ZERO)..)
        .take_while(|((h, _), ())| *h == height)
        .filter_map(|((_, id), ())| courses.session(id))
        .map(|s| {
            // An overflowing fee makes `end_session` reject the block, so the value is irrelevant.
            let fee = cancel_fee(s.locked).unwrap_or(s.locked);
            let record = AutoCancelRecord {
                session_id: s.id,
                owner: s.owner,
                locked: s.locked,
                fee,
                restored: s.locked.saturating_sub(fee),
            };
            (s.clone(), record)
        })
        .collect();
    let encoded: Vec<Vec<u8>> = ending.iter().map(|(_, r)| r.encode()).collect();
    let commitment = RecordCommitment {
        height,
        count: u32::try_from(ending.len()).unwrap_or(u32::MAX),
        records_root: merkle_root(&encoded),
    };
    (commitment, ending)
}

/// The three system transactions a block at `height` must start with (BLK-09).
#[must_use]
pub fn system_transactions(
    state: &ChainState,
    height: Height,
    beneficiary: PubKey,
    extra: BoundedBytes<MAX_COINBASE_EXTRA>,
) -> [Tx; 3] {
    [
        Tx::Expire(expire_commitment(state, height).0),
        Tx::AutoCancel(auto_cancel_commitment(state, height).0),
        Tx::Coinbase(Coinbase {
            height,
            beneficiary,
            extra,
        }),
    ]
}

fn check_body_structure(block: &Block) -> Result<(), BlockError> {
    if block.encode().len() > MAX_BLOCK_SIZE {
        return Err(BlockError::TooLarge);
    }
    if block.header.tx_root != block.compute_tx_root() {
        return Err(BlockError::TxRootMismatch);
    }
    let mut seen = BTreeSet::new();
    if !block.txs.iter().all(|tx| seen.insert(tx.txid())) {
        return Err(BlockError::DuplicateTx);
    }
    Ok(())
}

/// Validates `block` on top of `parent` and returns the state changes it makes, without
/// modifying `state` (BLK-01 – BLK-13).
///
/// `state` must be the state after `parent`, and `ancestry` must describe the branch ending at
/// `parent`.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn validate_block<A: Ancestry>(
    state: &ChainState,
    block: &Block,
    parent: &BlockHeader,
    chain: &ChainParams,
    ancestry: &A,
) -> Result<BlockDelta, BlockError> {
    validate_header(&block.header, parent, chain, ancestry)?;
    validate_body(state, block, chain)
}

/// BLK-01 and BLK-09 – BLK-13: everything except the header rules. Useful for checking a block
/// template before searching for a nonce; a block is valid only if [`validate_block`] accepts it.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn validate_body(
    state: &ChainState,
    block: &Block,
    chain: &ChainParams,
) -> Result<BlockDelta, BlockError> {
    check_body_structure(block)?;

    let height = block.header.height;
    let [
        Tx::Expire(expire),
        Tx::AutoCancel(auto_cancel),
        Tx::Coinbase(coinbase),
        user_txs @ ..,
    ] = block.txs.as_slice()
    else {
        return Err(BlockError::BadLayout);
    };
    if user_txs.iter().any(Tx::is_system) {
        return Err(BlockError::BadLayout);
    }
    let mut per_sender: BTreeMap<PubKey, u32> = BTreeMap::new();
    for sender in user_txs.iter().filter_map(Tx::sender) {
        let count = per_sender.entry(sender).or_insert(0);
        *count = count.saturating_add(1);
        if *count > MAX_USER_TXS_PER_SENDER_PER_BLOCK {
            return Err(BlockError::TooManyFromSender(sender));
        }
    }

    let mut overlay = Overlay::new(state);

    if expire.height != height {
        return Err(BlockError::ExpireHeight);
    }
    let (expected_expire, records) = expire_commitment(state, height);
    if *expire != expected_expire {
        return Err(BlockError::ExpireMismatch);
    }
    for record in &records {
        overlay.remove(record.outpoint)?;
        overlay.totals.expired = overlay
            .totals
            .expired
            .checked_add(record.utxo.amount)
            .ok_or(BlockError::Overflow)?;
    }

    if auto_cancel.height != height {
        return Err(BlockError::AutoCancelHeight);
    }
    let (expected_auto_cancel, ending) = auto_cancel_commitment(state, height);
    if *auto_cancel != expected_auto_cancel {
        return Err(BlockError::AutoCancelMismatch);
    }
    let auto_cancel_txid = block
        .txs
        .as_slice()
        .get(1)
        .map(Tx::txid)
        .ok_or(BlockError::BadLayout)?;
    let mut auto_cancelled = Vec::with_capacity(ending.len());
    for (index, (session, record)) in (0_u32..).zip(ending) {
        let restored_at = OutPoint {
            txid: auto_cancel_txid,
            index,
        };
        end_session(&mut overlay, &session, restored_at, height)
            .map_err(|_| BlockError::Overflow)?;
        auto_cancelled.push(record);
    }

    apply_coinbase(&mut overlay, coinbase, height, block.txs.as_slice())?;

    for (offset, tx) in user_txs.iter().enumerate() {
        let index = offset.saturating_add(3);
        let position = u32::try_from(index).map_err(|_| BlockError::BadLayout)?;
        apply_user_tx(&mut overlay, tx, position, height, &chain.chain_id)
            .map_err(|error| BlockError::Tx { index, error })?;
    }

    Ok(overlay.finish(block.header.id(), height, records, auto_cancelled))
}

fn apply_coinbase(
    overlay: &mut Overlay<'_>,
    coinbase: &Coinbase,
    height: Height,
    txs: &[Tx],
) -> Result<(), BlockError> {
    if coinbase.height != height {
        return Err(BlockError::CoinbaseHeight);
    }
    if !is_valid_public_key(&coinbase.beneficiary.0) {
        return Err(BlockError::InvalidBeneficiary);
    }
    // Balance after Expire and AutoCancel; the cap is never exceeded, so this cannot underflow.
    let room = BALANCE_CAP.saturating_sub(overlay.balance(&coinbase.beneficiary));
    let amount = BLOCK_REWARD.min(room);
    if amount == Credit::ZERO {
        return Ok(());
    }
    let spendable_from = height
        .checked_add(COINBASE_MATURITY)
        .ok_or(BlockError::Overflow)?;
    let expire_height = spendable_from
        .checked_add(EXPIRY_BLOCKS)
        .ok_or(BlockError::Overflow)?;
    let txid = txs.get(2).map(Tx::txid).ok_or(BlockError::BadLayout)?;
    overlay.create(
        OutPoint { txid, index: 0 },
        Utxo {
            owner: coinbase.beneficiary,
            amount,
            source: Source::Mined,
            acquired_height: height,
            spendable_from,
            expire_height,
            hops: 0,
            renewals: 0,
        },
    )?;
    overlay.totals.minted = overlay
        .totals
        .minted
        .checked_add(amount)
        .ok_or(BlockError::Overflow)?;
    Ok(())
}

/// Validates a genesis block (`spec/02-block.md` §6). Genesis changes no state.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn validate_genesis(block: &Block, chain: &ChainParams) -> Result<BlockDelta, BlockError> {
    validate_genesis_header(&block.header, chain)?;
    check_body_structure(block)?;
    let [Tx::Coinbase(coinbase)] = block.txs.as_slice() else {
        return Err(BlockError::InvalidGenesisBody);
    };
    if coinbase.height != Height(0) || coinbase.beneficiary != PubKey([0; 32]) {
        return Err(BlockError::InvalidGenesisBody);
    }
    let empty = ChainState::default();
    Ok(Overlay::new(&empty).finish(block.header.id(), Height(0), Vec::new(), Vec::new()))
}

/// Assembles an unsolved block (`nonce = 0`) from a header template and its transactions.
#[must_use]
pub fn assemble(mut header: BlockHeader, txs: Vec<Tx>) -> Option<Block> {
    let txs = BoundedVec::new(txs).ok()?;
    let txids: Vec<Hash> = txs.iter().map(Tx::txid).collect();
    header.tx_root = merkle_root(&txids);
    Some(Block { header, txs })
}
