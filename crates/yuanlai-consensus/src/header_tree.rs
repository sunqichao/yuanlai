use core::fmt;
use std::collections::BTreeMap;

use yuanlai_types::{BlockHeader, ChainParams, Hash, Height, Target, Timestamp, U256};

use crate::{
    Ancestry, ContextError, HeaderError, block_work, next_target, validate_genesis_header,
    validate_header,
};

/// Why a header could not be added to the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeError {
    /// BLK-04: the parent is not in the tree.
    UnknownParent(Hash),
    /// The header violates a header rule.
    Invalid(HeaderError),
    /// Cumulative work overflowed 256 bits (unreachable with targets ≤ `POW_LIMIT`).
    WorkOverflow,
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownParent(h) => write!(f, "BLK-04: unknown parent {h}"),
            Self::Invalid(e) => e.fmt(f),
            Self::WorkOverflow => f.write_str("cumulative chain work overflow"),
        }
    }
}

impl core::error::Error for TreeError {}

impl From<HeaderError> for TreeError {
    fn from(e: HeaderError) -> Self {
        Self::Invalid(e)
    }
}

/// Result of inserting a valid header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The header was already present.
    AlreadyKnown,
    /// Stored, but its branch has no more work than the current best (FORK-01, FORK-02).
    SideBranch,
    /// The header became the new best tip.
    NewBest {
        /// The previous best tip.
        previous: Hash,
        /// Last block shared by the previous and new best chains. Equals `previous` when the
        /// best chain was simply extended; otherwise blocks above it must be undone (FORK-03).
        fork_point: Hash,
    },
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    header: BlockHeader,
    chain_work: U256,
}

/// All known valid headers, with the best tip chosen by cumulative work.
///
/// In-memory only; persistence is added with the state machine in P1.
#[derive(Debug, Clone)]
pub struct HeaderTree {
    chain: ChainParams,
    entries: BTreeMap<Hash, Entry>,
    genesis: Hash,
    best: Hash,
}

impl HeaderTree {
    /// Creates a tree rooted at `genesis`.
    ///
    /// # Errors
    ///
    /// Returns an error if `genesis` violates the genesis header rules.
    pub fn new(chain: ChainParams, genesis: BlockHeader) -> Result<Self, TreeError> {
        validate_genesis_header(&genesis, &chain)?;
        let work = block_work(genesis.target).ok_or(TreeError::WorkOverflow)?;
        let id = genesis.id();
        let mut entries = BTreeMap::new();
        entries.insert(
            id,
            Entry {
                header: genesis,
                chain_work: work,
            },
        );
        Ok(Self {
            chain,
            entries,
            genesis: id,
            best: id,
        })
    }

    /// Validates `header` against its branch and stores it.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent is unknown or a header rule is violated. Invalid headers
    /// are not stored.
    pub fn insert(&mut self, header: BlockHeader) -> Result<InsertOutcome, TreeError> {
        let id = header.id();
        if self.entries.contains_key(&id) {
            return Ok(InsertOutcome::AlreadyKnown);
        }
        let parent = *self
            .entries
            .get(&header.prev_block)
            .ok_or(TreeError::UnknownParent(header.prev_block))?;

        validate_header(
            &header,
            &parent.header,
            &self.chain,
            &self.branch(header.prev_block),
        )?;

        let work = block_work(header.target).ok_or(TreeError::WorkOverflow)?;
        let chain_work = parent
            .chain_work
            .checked_add(work)
            .ok_or(TreeError::WorkOverflow)?;
        self.entries.insert(id, Entry { header, chain_work });

        let best_work = self.entry(self.best).map_or(U256::ZERO, |e| e.chain_work);
        // Strictly greater: on a tie the first-seen tip stays (FORK-02).
        if chain_work <= best_work {
            return Ok(InsertOutcome::SideBranch);
        }
        let previous = self.best;
        let fork_point = self.fork_point(previous, id);
        self.best = id;
        Ok(InsertOutcome::NewBest {
            previous,
            fork_point,
        })
    }

    /// The genesis block ID.
    #[must_use]
    pub const fn genesis_id(&self) -> Hash {
        self.genesis
    }

    /// The best tip's ID.
    #[must_use]
    pub const fn best_id(&self) -> Hash {
        self.best
    }

    /// The best tip's header.
    #[must_use]
    pub fn best_header(&self) -> Option<&BlockHeader> {
        self.entry(self.best).map(|e| &e.header)
    }

    /// A stored header by ID.
    #[must_use]
    pub fn header(&self, id: Hash) -> Option<&BlockHeader> {
        self.entry(id).map(|e| &e.header)
    }

    /// The last common ancestor of two stored headers.
    #[must_use]
    pub fn common_ancestor(&self, a: Hash, b: Hash) -> Option<Hash> {
        if self.entry(a).is_none() || self.entry(b).is_none() {
            return None;
        }
        Some(self.fork_point(a, b))
    }

    /// Headers after `ancestor` through `tip`, in forward chain order.
    ///
    /// Returns `None` if either ID is unknown or `ancestor` is not an ancestor of `tip`.
    #[must_use]
    pub fn headers_after(&self, ancestor: Hash, tip: Hash) -> Option<Vec<BlockHeader>> {
        self.entry(ancestor)?;
        let mut current = tip;
        let mut headers = Vec::new();
        while current != ancestor {
            if current == self.genesis {
                return None;
            }
            let entry = self.entry(current)?;
            headers.push(entry.header);
            current = entry.header.prev_block;
        }
        headers.reverse();
        Some(headers)
    }

    /// Cumulative work from genesis to `id`, inclusive.
    #[must_use]
    pub fn chain_work(&self, id: Hash) -> Option<U256> {
        self.entry(id).map(|e| e.chain_work)
    }

    /// Number of stored headers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether only genesis is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.len() <= 1
    }

    /// Read access to the branch ending at `tip`.
    #[must_use]
    pub const fn branch(&self, tip: Hash) -> Branch<'_> {
        Branch { tree: self, tip }
    }

    /// A header template on top of the best tip with the required target and `nonce = 0`.
    ///
    /// The caller chooses `timestamp` and must still satisfy BLK-07/BLK-08 and find a nonce.
    ///
    /// # Errors
    ///
    /// Returns a [`ContextError`] if the tree is internally inconsistent.
    pub fn next_template(
        &self,
        tx_root: Hash,
        timestamp: Timestamp,
    ) -> Result<BlockHeader, ContextError> {
        let tip = self
            .best_header()
            .ok_or(ContextError::MissingAncestor(Height(0)))?;
        let height = tip
            .height
            .checked_add(1)
            .ok_or(ContextError::MissingAncestor(tip.height))?;
        let target: Target = next_target(
            height,
            tip.target,
            self.chain.pow_limit,
            &self.branch(self.best),
        )?;
        Ok(BlockHeader {
            version: tip.version,
            chain_id: self.chain.chain_id,
            height,
            prev_block: self.best,
            tx_root,
            timestamp,
            target,
            nonce: 0,
        })
    }

    fn entry(&self, id: Hash) -> Option<&Entry> {
        self.entries.get(&id)
    }

    fn parent_of(&self, id: Hash) -> Option<(Hash, Height)> {
        let header = &self.entry(id)?.header;
        let parent = header.prev_block;
        let height = self.entry(parent)?.header.height;
        Some((parent, height))
    }

    /// Last common ancestor of `a` and `b`. Both must be stored.
    fn fork_point(&self, a: Hash, b: Hash) -> Hash {
        let height_of = |id: Hash| self.entry(id).map_or(Height(0), |e| e.header.height);
        let (mut a, mut b) = (a, b);
        while a != b {
            let (ha, hb) = (height_of(a), height_of(b));
            let step = |id: Hash| self.parent_of(id).map_or(self.genesis, |(p, _)| p);
            if ha >= hb {
                a = step(a);
            }
            if hb >= ha {
                b = step(b);
            }
        }
        a
    }
}

/// The branch of a [`HeaderTree`] ending at a particular tip.
#[derive(Debug, Clone, Copy)]
pub struct Branch<'a> {
    tree: &'a HeaderTree,
    tip: Hash,
}

impl Ancestry for Branch<'_> {
    fn timestamp_at(&self, height: Height) -> Option<Timestamp> {
        let mut entry = self.tree.entry(self.tip)?;
        while entry.header.height > height {
            entry = self.tree.entry(entry.header.prev_block)?;
        }
        (entry.header.height == height).then_some(entry.header.timestamp)
    }
}
