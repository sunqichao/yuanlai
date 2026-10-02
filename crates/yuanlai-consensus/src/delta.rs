use std::collections::{BTreeMap, BTreeSet};

use yuanlai_types::{AutoCancelRecord, Credit, ExpireRecord, Hash, Height, OutPoint, PubKey, Utxo};

use crate::course::{CourseDelta, CourseJournal};
use crate::state::{AuthorVerification, ChainState, Totals};

/// One accepted transfer, as recorded for TX-TRANSFER-05 and TX-TRANSFER-07.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferEntry {
    /// Sender.
    pub from: PubKey,
    /// Recipient.
    pub to: PubKey,
    /// Credit received after the loss.
    pub received: Credit,
}

/// All state changes made by one block: its undo record (`spec/02-block.md` FORK-03).
///
/// Applying is infallible because the delta was computed against the exact state it is applied
/// to; reverting restores that state bit for bit (INV-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDelta {
    /// Block that produced this delta.
    pub block_id: Hash,
    /// Its height.
    pub height: Height,
    /// UTXOs removed, with their full data.
    pub removed: Vec<(OutPoint, Utxo)>,
    /// UTXOs created.
    pub created: Vec<(OutPoint, Utxo)>,
    /// Balances touched: `(owner, before, after)`.
    pub balances: Vec<(PubKey, Credit, Credit)>,
    /// Totals before the block.
    pub totals_before: Totals,
    /// Totals after the block.
    pub totals_after: Totals,
    /// Expiry records of this block, kept for queries.
    pub expired: Vec<ExpireRecord>,
    /// Account sequence numbers touched: `(owner, before, after)`.
    pub nonces: Vec<(PubKey, u64, u64)>,
    /// Identities registered in this block.
    pub identities: Vec<(PubKey, Hash)>,
    /// Successful author verifications in this block.
    pub author_verifications: Vec<AuthorVerification>,
    /// Transfers accepted in this block.
    pub transfers: Vec<TransferEntry>,
    /// Course and session changes.
    pub courses: CourseDelta,
}

fn set_nonce(nonces: &mut BTreeMap<PubKey, u64>, owner: PubKey, value: u64) {
    if value == 0 {
        nonces.remove(&owner);
    } else {
        nonces.insert(owner, value);
    }
}

fn set_balance(balances: &mut BTreeMap<PubKey, Credit>, owner: PubKey, value: Credit) {
    if value == Credit::ZERO {
        balances.remove(&owner);
    } else {
        balances.insert(owner, value);
    }
}

impl ChainState {
    pub(crate) fn apply(&mut self, delta: &BlockDelta) {
        for (outpoint, utxo) in &delta.removed {
            self.utxos.remove(outpoint);
            self.expiry_index.remove(&(utxo.expire_height, *outpoint));
        }
        for (outpoint, utxo) in &delta.created {
            self.utxos.insert(*outpoint, *utxo);
            self.expiry_index.insert((utxo.expire_height, *outpoint));
        }
        for (owner, _, after) in &delta.balances {
            set_balance(&mut self.balances, *owner, *after);
        }
        for (owner, _, after) in &delta.nonces {
            set_nonce(&mut self.nonces, *owner, *after);
        }
        for (owner, commitment) in &delta.identities {
            self.identities.insert(*owner, *commitment);
            self.identity_owners.insert(*commitment, *owner);
        }
        for verification in &delta.author_verifications {
            self.author_verifications
                .insert(verification.nullifier, *verification);
        }
        for t in &delta.transfers {
            let count = self
                .transfer_pairs
                .entry((t.from, t.to, delta.height))
                .or_insert(0);
            *count = count.saturating_add(1);
            let inbound = self
                .inbound
                .entry((t.to, delta.height))
                .or_insert(Credit::ZERO);
            // Bounded by INBOUND_CAP, which validation already checked.
            *inbound = inbound.checked_add(t.received).unwrap_or(*inbound);
        }
        self.courses.replay(&delta.courses, true);
        self.totals = delta.totals_after;
    }

    pub(crate) fn revert(&mut self, delta: &BlockDelta) {
        for (outpoint, utxo) in &delta.created {
            self.utxos.remove(outpoint);
            self.expiry_index.remove(&(utxo.expire_height, *outpoint));
        }
        for (outpoint, utxo) in &delta.removed {
            self.utxos.insert(*outpoint, *utxo);
            self.expiry_index.insert((utxo.expire_height, *outpoint));
        }
        for (owner, before, _) in &delta.balances {
            set_balance(&mut self.balances, *owner, *before);
        }
        for (owner, before, _) in &delta.nonces {
            set_nonce(&mut self.nonces, *owner, *before);
        }
        for (owner, commitment) in &delta.identities {
            self.identities.remove(owner);
            self.identity_owners.remove(commitment);
        }
        for verification in &delta.author_verifications {
            self.author_verifications.remove(&verification.nullifier);
        }
        for t in &delta.transfers {
            let pair = (t.from, t.to, delta.height);
            match self.transfer_pairs.get(&pair).copied() {
                Some(n) if n > 1 => {
                    self.transfer_pairs.insert(pair, n.saturating_sub(1));
                }
                _ => {
                    self.transfer_pairs.remove(&pair);
                }
            }
            let key = (t.to, delta.height);
            let remaining = self
                .inbound
                .get(&key)
                .map_or(Credit::ZERO, |v| v.saturating_sub(t.received));
            if remaining == Credit::ZERO {
                self.inbound.remove(&key);
            } else {
                self.inbound.insert(key, remaining);
            }
        }
        self.courses.replay(&delta.courses, false);
        self.totals = delta.totals_before;
    }
}

/// Pending changes of the block being validated, layered over the unmodified base state.
pub(crate) struct Overlay<'a> {
    base: &'a ChainState,
    removed: BTreeMap<OutPoint, Utxo>,
    created: BTreeMap<OutPoint, Utxo>,
    balances: BTreeMap<PubKey, (Credit, Credit)>,
    nonces: BTreeMap<PubKey, (u64, u64)>,
    identities: BTreeMap<PubKey, Hash>,
    commitments: BTreeSet<Hash>,
    author_verifications: BTreeMap<Hash, AuthorVerification>,
    transfers: Vec<TransferEntry>,
    pub(crate) courses: CourseJournal<'a>,
    pub(crate) totals: Totals,
}

/// A credit sum left the `u64` range. Unreachable under the balance cap; treated as invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Overflow;

impl<'a> Overlay<'a> {
    pub(crate) fn new(base: &'a ChainState) -> Self {
        Self {
            base,
            removed: BTreeMap::new(),
            created: BTreeMap::new(),
            balances: BTreeMap::new(),
            nonces: BTreeMap::new(),
            identities: BTreeMap::new(),
            commitments: BTreeSet::new(),
            author_verifications: BTreeMap::new(),
            transfers: Vec::new(),
            courses: CourseJournal::new(&base.courses),
            totals: base.totals,
        }
    }

    /// A UTXO as seen by this block so far.
    pub(crate) fn utxo(&self, outpoint: &OutPoint) -> Option<Utxo> {
        if let Some(u) = self.created.get(outpoint) {
            return Some(*u);
        }
        if self.removed.contains_key(outpoint) {
            return None;
        }
        self.base.utxo(outpoint).copied()
    }

    pub(crate) fn nonce(&self, owner: &PubKey) -> u64 {
        self.nonces
            .get(owner)
            .map_or_else(|| self.base.nonce(owner), |(_, after)| *after)
    }

    pub(crate) fn set_nonce(&mut self, owner: PubKey, value: u64) {
        let before = self.base.nonce(&owner);
        self.nonces.insert(owner, (before, value));
    }

    pub(crate) fn identity(&self, owner: &PubKey) -> Option<Hash> {
        self.identities
            .get(owner)
            .copied()
            .or_else(|| self.base.identity(owner))
    }

    pub(crate) fn commitment_used(&self, commitment: &Hash) -> bool {
        self.commitments.contains(commitment) || self.base.identity_owner(commitment).is_some()
    }

    pub(crate) fn add_identity(&mut self, owner: PubKey, commitment: Hash) {
        self.identities.insert(owner, commitment);
        self.commitments.insert(commitment);
    }

    pub(crate) fn nullifier_used(&self, nullifier: &Hash) -> bool {
        self.author_verifications.contains_key(nullifier)
            || self.base.author_verifications.contains_key(nullifier)
    }

    pub(crate) fn add_author_verification(&mut self, verification: AuthorVerification) {
        self.author_verifications
            .insert(verification.nullifier, verification);
    }

    /// Whether `from` transferred to `to` in `[lo, current block]`, including this block.
    pub(crate) fn transferred_since(
        &self,
        from: PubKey,
        to: PubKey,
        lo: Height,
        current: Height,
    ) -> bool {
        self.transfers.iter().any(|t| t.from == from && t.to == to)
            || self.base.transferred_between(from, to, lo, current)
    }

    /// Credit `to` received by transfer in `[lo, current block]`, including this block.
    pub(crate) fn inbound_since(&self, to: PubKey, lo: Height, current: Height) -> Option<Credit> {
        self.transfers
            .iter()
            .filter(|t| t.to == to)
            .try_fold(self.base.inbound_between(to, lo, current)?, |acc, t| {
                acc.checked_add(t.received)
            })
    }

    pub(crate) fn add_transfer(&mut self, entry: TransferEntry) {
        self.transfers.push(entry);
    }

    pub(crate) fn balance(&self, owner: &PubKey) -> Credit {
        self.balances
            .get(owner)
            .map_or_else(|| self.base.balance(owner), |(_, after)| *after)
    }

    fn adjust_balance(
        &mut self,
        owner: PubKey,
        f: impl FnOnce(Credit) -> Option<Credit>,
    ) -> Result<(), Overflow> {
        let before = self.base.balance(&owner);
        let current = self.balance(&owner);
        let after = f(current).ok_or(Overflow)?;
        self.balances.insert(owner, (before, after));
        Ok(())
    }

    /// Removes an existing UTXO. Returns `None` if it does not exist in the overlaid state.
    pub(crate) fn remove(&mut self, outpoint: OutPoint) -> Result<Option<Utxo>, Overflow> {
        let utxo = if let Some(utxo) = self.created.remove(&outpoint) {
            utxo
        } else if self.removed.contains_key(&outpoint) {
            return Ok(None);
        } else if let Some(utxo) = self.base.utxo(&outpoint) {
            self.removed.insert(outpoint, *utxo);
            *utxo
        } else {
            return Ok(None);
        };
        self.adjust_balance(utxo.owner, |b| b.checked_sub(utxo.amount))?;
        Ok(Some(utxo))
    }

    pub(crate) fn create(&mut self, outpoint: OutPoint, utxo: Utxo) -> Result<(), Overflow> {
        self.adjust_balance(utxo.owner, |b| b.checked_add(utxo.amount))?;
        self.created.insert(outpoint, utxo);
        Ok(())
    }

    /// Moves `amount` into a session: still counted in the owner's balance (`spec/04-credit.md` §5).
    pub(crate) fn lock(&mut self, owner: PubKey, amount: Credit) -> Result<(), Overflow> {
        self.adjust_balance(owner, |b| b.checked_add(amount))
    }

    /// Removes `amount` of locked credit from the owner's balance when a session ends.
    pub(crate) fn unlock(&mut self, owner: PubKey, amount: Credit) -> Result<(), Overflow> {
        self.adjust_balance(owner, |b| b.checked_sub(amount))
    }

    pub(crate) fn finish(
        self,
        block_id: Hash,
        height: Height,
        expired: Vec<ExpireRecord>,
        auto_cancelled: Vec<AutoCancelRecord>,
    ) -> BlockDelta {
        BlockDelta {
            block_id,
            height,
            courses: self.courses.finish(auto_cancelled),
            removed: self.removed.into_iter().collect(),
            created: self.created.into_iter().collect(),
            balances: self
                .balances
                .into_iter()
                .filter(|(_, (before, after))| before != after)
                .map(|(owner, (before, after))| (owner, before, after))
                .collect(),
            totals_before: self.base.totals,
            totals_after: self.totals,
            expired,
            nonces: self
                .nonces
                .into_iter()
                .map(|(owner, (before, after))| (owner, before, after))
                .collect(),
            identities: self.identities.into_iter().collect(),
            author_verifications: self.author_verifications.into_values().collect(),
            transfers: self.transfers,
        }
    }
}
