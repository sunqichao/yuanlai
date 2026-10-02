use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use yuanlai_types::params::{BALANCE_CAP, MAX_HOPS, MAX_RENEWALS};
use yuanlai_types::{Credit, ExpireRecord, Hash, Height, OutPoint, PubKey, Utxo};

use crate::course::{CourseState, lock_bounds};

/// A successful author verification retained by nullifier for queries and reorg undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorVerification {
    /// Author address that proved knowledge of the registered identity secret.
    pub sender: PubKey,
    /// Registered course root bound by the proof.
    pub course_root: Hash,
    /// Verifier-issued challenge bound by the proof.
    pub challenge: Hash,
    /// One-time nullifier preventing replay.
    pub nullifier: Hash,
    /// Block height that recorded the verification.
    pub height: Height,
}

/// Running totals behind the conservation invariant INV-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    /// All credit ever issued by Coinbase.
    pub minted: Credit,
    /// All credit destroyed by fees and transfer losses.
    pub burned: Credit,
    /// All credit removed by `Expire`.
    pub expired: Credit,
}

/// Consensus state after the last connected block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainState {
    pub(crate) utxos: BTreeMap<OutPoint, Utxo>,
    /// `(expire_height, outpoint)` for every UTXO; iteration order is the spec's expiry order.
    pub(crate) expiry_index: BTreeSet<(Height, OutPoint)>,
    /// Non-zero balances only.
    pub(crate) balances: BTreeMap<PubKey, Credit>,
    pub(crate) totals: Totals,
    /// Account sequence numbers; absent means 0.
    pub(crate) nonces: BTreeMap<PubKey, u64>,
    pub(crate) identities: BTreeMap<PubKey, Hash>,
    pub(crate) identity_owners: BTreeMap<Hash, PubKey>,
    /// Successful author verifications indexed by one-time nullifier.
    pub(crate) author_verifications: BTreeMap<Hash, AuthorVerification>,
    /// Number of transfers per `(from, to, height)`, for TX-TRANSFER-05.
    pub(crate) transfer_pairs: BTreeMap<(PubKey, PubKey, Height), u32>,
    /// Credit received by transfer per `(recipient, height)`, for TX-TRANSFER-07.
    pub(crate) inbound: BTreeMap<(PubKey, Height), Credit>,
    pub(crate) courses: CourseState,
}

/// First height of the window `(h − w, h]`.
pub(crate) fn window_start(h: Height, w: u64) -> Height {
    Height(h.0.checked_sub(w).map_or(0, |v| v.saturating_add(1)))
}

impl ChainState {
    /// A UTXO by outpoint.
    #[must_use]
    pub fn utxo(&self, outpoint: &OutPoint) -> Option<&Utxo> {
        self.utxos.get(outpoint)
    }

    /// All UTXOs in outpoint order.
    pub fn utxos(&self) -> impl Iterator<Item = (&OutPoint, &Utxo)> {
        self.utxos.iter()
    }

    /// `balance(a)` from `spec/04-credit.md` §5.
    #[must_use]
    pub fn balance(&self, owner: &PubKey) -> Credit {
        self.balances.get(owner).copied().unwrap_or(Credit::ZERO)
    }

    /// Running totals.
    #[must_use]
    pub const fn totals(&self) -> Totals {
        self.totals
    }

    /// Account sequence number the next user transaction of `owner` must carry.
    #[must_use]
    pub fn nonce(&self, owner: &PubKey) -> u64 {
        self.nonces.get(owner).copied().unwrap_or(0)
    }

    /// Courses and open sessions.
    #[must_use]
    pub const fn courses(&self) -> &CourseState {
        &self.courses
    }

    /// Registered identity commitment of `owner`.
    #[must_use]
    pub fn identity(&self, owner: &PubKey) -> Option<Hash> {
        self.identities.get(owner).copied()
    }

    /// Address that registered `commitment`.
    #[must_use]
    pub fn identity_owner(&self, commitment: &Hash) -> Option<PubKey> {
        self.identity_owners.get(commitment).copied()
    }

    /// Author verification that consumed `nullifier`, if any.
    #[must_use]
    pub fn author_verification(&self, nullifier: &Hash) -> Option<&AuthorVerification> {
        self.author_verifications.get(nullifier)
    }

    /// All successful author verifications, ordered by nullifier.
    pub fn author_verifications(&self) -> impl Iterator<Item = &AuthorVerification> {
        self.author_verifications.values()
    }

    /// Whether `from` transferred to `to` in any block with height in `[lo, hi]`.
    #[must_use]
    pub fn transferred_between(&self, from: PubKey, to: PubKey, lo: Height, hi: Height) -> bool {
        lo <= hi
            && self
                .transfer_pairs
                .range((from, to, lo)..=(from, to, hi))
                .next()
                .is_some()
    }

    /// Credit `to` received by transfer in blocks with height in `[lo, hi]`.
    #[must_use]
    pub fn inbound_between(&self, to: PubKey, lo: Height, hi: Height) -> Option<Credit> {
        if lo > hi {
            return Some(Credit::ZERO);
        }
        self.inbound
            .range((to, lo)..=(to, hi))
            .try_fold(Credit::ZERO, |acc, (_, v)| acc.checked_add(*v))
    }

    /// Wallet helper: the inputs that satisfy TX-IN-02 – TX-IN-04 for taking `amount` from
    /// `owner` at `height`, restricted to UTXOs accepted by `filter`. `None` if not enough credit.
    #[must_use]
    pub fn select_inputs(
        &self,
        owner: &PubKey,
        amount: Credit,
        height: Height,
        filter: impl Fn(&Utxo) -> bool,
    ) -> Option<Vec<OutPoint>> {
        let mut candidates: Vec<(Height, OutPoint, Credit)> = self
            .utxos
            .iter()
            .filter(|(_, u)| u.owner == *owner && u.is_spendable_at(height) && filter(u))
            .map(|(o, u)| (u.expire_height, *o, u.amount))
            .collect();
        candidates.sort_unstable();
        let mut selected = Vec::new();
        let mut sum = Credit::ZERO;
        for (_, outpoint, value) in candidates {
            if sum >= amount {
                break;
            }
            selected.push(outpoint);
            sum = sum.checked_add(value)?;
        }
        (sum >= amount && amount > Credit::ZERO).then_some(selected)
    }

    /// Records for the UTXOs that expire at `height`, in spec order (`spec/04-credit.md` §6).
    #[must_use]
    pub fn expiring_at(&self, height: Height) -> Vec<ExpireRecord> {
        let start = (
            height,
            OutPoint {
                txid: Hash::ZERO,
                index: 0,
            },
        );
        self.expiry_index
            .range(start..)
            .take_while(|(h, _)| *h == height)
            .filter_map(|(_, outpoint)| {
                self.utxos.get(outpoint).map(|utxo| ExpireRecord {
                    outpoint: *outpoint,
                    utxo: *utxo,
                })
            })
            .collect()
    }

    /// Recomputes every derived structure from the UTXO set and checks the invariants that can be
    /// verified from state alone, after connecting the block at `height`.
    ///
    /// # Errors
    ///
    /// Returns every violated invariant.
    pub fn audit(&self, height: Height) -> Result<(), Vec<Violation>> {
        let mut violations = Vec::new();
        let mut balances: BTreeMap<PubKey, Credit> = BTreeMap::new();
        let mut index = BTreeSet::new();
        let mut unspent = Credit::ZERO;

        for (outpoint, utxo) in &self.utxos {
            if utxo.amount == Credit::ZERO {
                violations.push(Violation::Inv02ZeroAmount(*outpoint));
            }
            if utxo.expire_height <= height {
                violations.push(Violation::Inv03NotExpired(*outpoint));
            }
            let entry = balances.entry(utxo.owner).or_insert(Credit::ZERO);
            match (
                entry.checked_add(utxo.amount),
                unspent.checked_add(utxo.amount),
            ) {
                (Some(b), Some(u)) => {
                    *entry = b;
                    unspent = u;
                }
                _ => violations.push(Violation::Overflow),
            }
            index.insert((utxo.expire_height, *outpoint));
        }

        let locked = self.audit_sessions(height, &mut balances, &mut violations);

        for (owner, balance) in &balances {
            if *balance > BALANCE_CAP {
                violations.push(Violation::Inv04BalanceCap(*owner));
            }
        }
        if balances != self.balances {
            violations.push(Violation::BalanceCacheMismatch);
        }
        if index != self.expiry_index {
            violations.push(Violation::ExpiryIndexMismatch);
        }
        let owners_consistent = self.identities.len() == self.identity_owners.len()
            && self
                .identities
                .iter()
                .all(|(owner, c)| self.identity_owners.get(c) == Some(owner));
        if !owners_consistent {
            violations.push(Violation::Inv11Identity);
        }

        let t = self.totals;
        let accounted = unspent
            .checked_add(locked)
            .and_then(|v| v.checked_add(t.burned))
            .and_then(|v| v.checked_add(t.expired));
        if accounted != Some(t.minted) {
            violations.push(Violation::Inv01Conservation);
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    /// Checks INV-05 – INV-08 for open sessions, adds locked credit to `balances`, and returns the
    /// total locked credit.
    fn audit_sessions(
        &self,
        height: Height,
        balances: &mut BTreeMap<PubKey, Credit>,
        violations: &mut Vec<Violation>,
    ) -> Credit {
        let mut locked = Credit::ZERO;
        let mut open_by_owner = BTreeMap::new();
        #[expect(
            clippy::zero_sized_map_values,
            reason = "compared with CourseState::deadlines"
        )]
        let mut deadlines = BTreeMap::new();
        for session in self.courses.sessions.values() {
            let entry = balances.entry(session.owner).or_insert(Credit::ZERO);
            match (
                entry.checked_add(session.locked),
                locked.checked_add(session.locked),
            ) {
                (Some(b), Some(l)) => {
                    *entry = b;
                    locked = l;
                }
                _ => violations.push(Violation::Overflow),
            }
            if session.max_hops > MAX_HOPS || session.max_renewals >= MAX_RENEWALS {
                violations.push(Violation::Inv05Session(session.id));
            }
            let in_bounds = session
                .estimate()
                .and_then(lock_bounds)
                .is_some_and(|(lo, hi)| lo <= session.locked && session.locked <= hi);
            if !in_bounds {
                violations.push(Violation::Inv06LockBounds(session.id));
            }
            if open_by_owner.insert(session.owner, session.id).is_some() {
                violations.push(Violation::Inv07OpenSessions(session.owner));
            }
            if session.deadline() <= height {
                violations.push(Violation::Inv08Deadline(session.id));
            }
            deadlines.insert((session.deadline(), session.id), ());
            let files = self.courses.session_files(&session.id);
            let size = files
                .iter()
                .try_fold(0_u64, |acc, f| acc.checked_add(f.size));
            if u32::try_from(files.len()).ok() != Some(session.file_count)
                || size != Some(session.total_size)
            {
                violations.push(Violation::SessionFilesMismatch(session.id));
            }
        }
        if open_by_owner != self.courses.open_by_owner || deadlines != self.courses.deadlines {
            violations.push(Violation::SessionIndexMismatch);
        }
        locked
    }
}

/// An invariant violation found by [`ChainState::audit`]. Any occurrence is a critical bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// INV-01
    Inv01Conservation,
    /// INV-02
    Inv02ZeroAmount(OutPoint),
    /// INV-03
    Inv03NotExpired(OutPoint),
    /// INV-04
    Inv04BalanceCap(PubKey),
    /// INV-05: a session carries credit beyond the hop or renewal limits.
    Inv05Session(Hash),
    /// INV-06
    Inv06LockBounds(Hash),
    /// INV-07
    Inv07OpenSessions(PubKey),
    /// INV-08
    Inv08Deadline(Hash),
    /// A session's file count or size differs from its stored files.
    SessionFilesMismatch(Hash),
    /// The open-session or deadline index differs from the sessions.
    SessionIndexMismatch,
    /// Cached balances differ from the UTXO set.
    BalanceCacheMismatch,
    /// Expiry index differs from the UTXO set.
    ExpiryIndexMismatch,
    /// INV-11: identities and commitments are not a one-to-one mapping.
    Inv11Identity,
    /// A sum overflowed.
    Overflow,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
