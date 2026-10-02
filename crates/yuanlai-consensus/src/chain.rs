use core::fmt;

use yuanlai_codec::BoundedBytes;
use yuanlai_types::params::{MAX_COINBASE_EXTRA, MTP_WINDOW, RETARGET_INTERVAL, TIMEWARP_GRACE};
use yuanlai_types::{Block, BlockHeader, ChainParams, Hash, Height, PubKey, Timestamp, Tx};

use crate::block::{
    assemble, system_transactions, validate_block, validate_body, validate_genesis,
};
use crate::{Ancestry, BlockDelta, BlockError, ChainState, median_time_past, next_target};

/// Why an active chain could not be replaced by a candidate branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReorgError {
    /// The requested fork point is above the active tip.
    ForkAboveTip,
    /// A branch replacement must contain at least one block after the fork point.
    EmptyBranch,
    /// A candidate block failed full consensus validation.
    InvalidBlock(BlockError),
}

impl fmt::Display for ReorgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForkAboveTip => formatter.write_str("fork point is above the active tip"),
            Self::EmptyBranch => formatter.write_str("replacement branch is empty"),
            Self::InvalidBlock(error) => {
                write!(formatter, "replacement block is invalid: {error}")
            }
        }
    }
}

impl core::error::Error for ReorgError {}

/// The best chain with full state and one undo record per block.
///
/// Connects and disconnects blocks at the tip. Competing branches are selected by
/// [`crate::HeaderTree`] and can be activated with [`Self::reorganize`].
#[derive(Debug, Clone)]
pub struct Chain {
    params: ChainParams,
    genesis: BlockHeader,
    /// Index = height; `headers[0]` is `genesis`.
    headers: Vec<BlockHeader>,
    deltas: Vec<BlockDelta>,
    state: ChainState,
}

impl Chain {
    /// Starts a chain from a genesis block.
    ///
    /// # Errors
    ///
    /// Returns an error if `genesis` is invalid for `params`.
    pub fn new(params: ChainParams, genesis: &Block) -> Result<Self, BlockError> {
        let delta = validate_genesis(genesis, &params)?;
        Ok(Self {
            params,
            genesis: genesis.header,
            headers: vec![genesis.header],
            deltas: vec![delta],
            state: ChainState::default(),
        })
    }

    /// Network constants.
    #[must_use]
    pub const fn params(&self) -> &ChainParams {
        &self.params
    }

    /// The tip header.
    #[must_use]
    pub fn tip(&self) -> &BlockHeader {
        self.headers.last().unwrap_or(&self.genesis)
    }

    /// The tip height.
    #[must_use]
    pub fn height(&self) -> Height {
        self.tip().height
    }

    /// State after the tip.
    #[must_use]
    pub const fn state(&self) -> &ChainState {
        &self.state
    }

    /// Checks the body rules of a block extending the tip, without its header rules (proof of
    /// work, timestamps). Lets a miner filter transactions before searching for a nonce.
    ///
    /// # Errors
    ///
    /// Returns the first violated body rule.
    pub fn check_body(&self, block: &Block) -> Result<(), BlockError> {
        validate_body(&self.state, block, &self.params).map(drop)
    }

    /// Header at `height` on this chain.
    #[must_use]
    pub fn header_at(&self, height: Height) -> Option<&BlockHeader> {
        self.headers.get(usize::try_from(height.0).ok()?)
    }

    /// Undo record (including expiry records) of the block at `height`.
    #[must_use]
    pub fn delta_at(&self, height: Height) -> Option<&BlockDelta> {
        self.deltas.get(usize::try_from(height.0).ok()?)
    }

    /// Validates `block` as a child of the tip and connects it. Its undo record is then available
    /// from [`Self::delta_at`].
    ///
    /// # Errors
    ///
    /// Returns the violated rule; the chain is unchanged.
    pub fn connect(&mut self, block: &Block) -> Result<(), BlockError> {
        let parent = *self.tip();
        let delta = validate_block(&self.state, block, &parent, &self.params, self)?;
        self.state.apply(&delta);
        self.headers.push(block.header);
        self.deltas.push(delta);
        Ok(())
    }

    /// Disconnects the tip, restoring the previous state exactly. Genesis cannot be disconnected.
    pub fn disconnect_tip(&mut self) -> Option<(BlockHeader, BlockDelta)> {
        if self.headers.len() <= 1 {
            return None;
        }
        let header = self.headers.pop()?;
        let delta = self.deltas.pop()?;
        self.state.revert(&delta);
        Some((header, delta))
    }

    /// Atomically replaces blocks above `fork_height` with a fully validated branch.
    ///
    /// The caller must only use this for the branch selected by the header tree's cumulative-work
    /// rule. The candidate is applied to a clone first, so an invalid block leaves this chain and
    /// its state untouched.
    ///
    /// # Errors
    ///
    /// Returns [`ReorgError::ForkAboveTip`] if the fork point exceeds the active tip,
    /// [`ReorgError::EmptyBranch`] if no replacement blocks are supplied, or
    /// [`ReorgError::InvalidBlock`] if any replacement block fails consensus validation.
    pub fn reorganize(&mut self, fork_height: Height, branch: &[Block]) -> Result<(), ReorgError> {
        if branch.is_empty() {
            return Err(ReorgError::EmptyBranch);
        }
        if fork_height > self.height() {
            return Err(ReorgError::ForkAboveTip);
        }

        let mut candidate = self.clone();
        while candidate.height() > fork_height {
            candidate.disconnect_tip().ok_or(ReorgError::ForkAboveTip)?;
        }
        for block in branch {
            candidate.connect(block).map_err(ReorgError::InvalidBlock)?;
        }
        *self = candidate;
        Ok(())
    }

    /// Smallest timestamp the next block may use (BLK-07 and BLK-08).
    #[must_use]
    pub fn min_next_timestamp(&self) -> Timestamp {
        let start = self.headers.len().saturating_sub(MTP_WINDOW);
        let window: Vec<Timestamp> = self
            .headers
            .get(start..)
            .unwrap_or_default()
            .iter()
            .map(|h| h.timestamp)
            .collect();
        let after_median = median_time_past(&window).map_or(0, |m| m.0.saturating_add(1));
        let next_height = self.height().0.saturating_add(1);
        let warp_floor = if next_height.checked_rem(RETARGET_INTERVAL) == Some(0) {
            self.tip().timestamp.0.saturating_sub(TIMEWARP_GRACE)
        } else {
            0
        };
        Timestamp(after_median.max(warp_floor))
    }

    /// An unsolved block (`nonce = 0`) extending the tip, containing only system transactions.
    ///
    /// Returns `None` if the chain cannot be extended (height overflow or inconsistent state).
    #[must_use]
    pub fn template(
        &self,
        beneficiary: PubKey,
        extra: BoundedBytes<MAX_COINBASE_EXTRA>,
        timestamp: Timestamp,
    ) -> Option<Block> {
        self.template_with(beneficiary, extra, timestamp, Vec::new())
    }

    /// Like [`Self::template`], followed by `user_txs`. The transactions are not validated here.
    #[must_use]
    pub fn template_with(
        &self,
        beneficiary: PubKey,
        extra: BoundedBytes<MAX_COINBASE_EXTRA>,
        timestamp: Timestamp,
        user_txs: Vec<Tx>,
    ) -> Option<Block> {
        let tip = self.tip();
        let height = tip.height.checked_add(1)?;
        let target = next_target(height, tip.target, self.params.pow_limit, self).ok()?;
        let header = BlockHeader {
            version: tip.version,
            chain_id: self.params.chain_id,
            height,
            prev_block: tip.id(),
            tx_root: Hash::ZERO,
            timestamp,
            target,
            nonce: 0,
        };
        let txs = system_transactions(&self.state, height, beneficiary, extra);
        let mut all: Vec<Tx> = txs.into();
        all.extend(user_txs);
        assemble(header, all)
    }
}

impl Ancestry for Chain {
    fn timestamp_at(&self, height: Height) -> Option<Timestamp> {
        self.header_at(height).map(|h| h.timestamp)
    }
}
