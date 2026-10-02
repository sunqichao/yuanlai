use core::fmt;
use std::collections::BTreeSet;

use yuanlai_codec::Encode;
use yuanlai_crypto::verify_signature;
use yuanlai_types::params::{
    BALANCE_CAP, CIRCUIT_FIELD_MODULUS, COST_IDENTITY, COST_VERIFY_AUTHOR, EXPIRY_BLOCKS,
    INBOUND_CAP, INBOUND_WINDOW, MAX_HOPS, MAX_TX_SIZE, NO_RETURN_WINDOW, TRANSFER_LOSS_PERCENT,
    TRANSFER_MIN_LOSS,
};
use yuanlai_types::{
    Credit, Hash, Height, OutPoint, PubKey, RegisterIdentity, Source, Transfer, Tx, U256, UserTx,
    Utxo, VerifyAuthor,
};
use yuanlai_zk::verify_author_consensus;

use crate::course_tx::{
    CourseError, apply_add_lock, apply_append, apply_begin, apply_cancel, apply_finalize,
};
use crate::delta::{Overflow, Overlay, TransferEntry};
use crate::state::{AuthorVerification, window_start};

/// A user transaction violates a rule of `spec/03-transactions.md` or `spec/04-credit.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxError {
    /// TX-SIZE-01
    TooLarge,
    /// TX-SIG-01
    BadSignature,
    /// TX-NONCE-01
    BadNonce {
        /// The sender's current sequence number.
        expected: u64,
        /// The transaction's nonce.
        found: u64,
    },
    /// TX-IN-01
    DuplicateInput,
    /// TX-IN-02: the output does not exist (or was already spent).
    MissingInput(OutPoint),
    /// TX-IN-02: the output belongs to someone else.
    NotOwner(OutPoint),
    /// TX-IN-02: the output is immature or expired.
    NotSpendable(OutPoint),
    /// TX-IN-03
    InputOrder,
    /// TX-IN-04
    InputsNotExact,
    /// TX-IN-05
    NoInputs,
    /// TX-TRANSFER-01
    AmountTooSmall,
    /// TX-TRANSFER-02
    TransferToSelf,
    /// TX-TRANSFER-03
    HopsExhausted,
    /// TX-TRANSFER-04
    RecipientWithoutIdentity,
    /// TX-TRANSFER-05
    ReturnTransfer,
    /// TX-TRANSFER-06
    RecipientBalanceCap,
    /// TX-TRANSFER-07
    RecipientInboundCap,
    /// TX-ID-01
    IdentityExists,
    /// TX-ID-02
    CommitmentUsed,
    /// TX-ID-03
    CommitmentOutOfField,
    /// TX-VA-01
    AuthorCourseNotOwned,
    /// TX-VA-02
    AuthorIdentityMissing,
    /// TX-VA-03
    NullifierUsed,
    /// TX-VA-04
    BadAuthorProof,
    /// A course rule (`spec/05-course-session.md`).
    Course(CourseError),
    /// A credit or height computation overflowed.
    Overflow,
}

impl TxError {
    /// The spec rule identifier this error corresponds to.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::TooLarge => "TX-SIZE-01",
            Self::BadSignature => "TX-SIG-01",
            Self::BadNonce { .. } => "TX-NONCE-01",
            Self::DuplicateInput => "TX-IN-01",
            Self::MissingInput(_) | Self::NotOwner(_) | Self::NotSpendable(_) => "TX-IN-02",
            Self::InputOrder => "TX-IN-03",
            Self::InputsNotExact => "TX-IN-04",
            Self::NoInputs => "TX-IN-05",
            Self::AmountTooSmall => "TX-TRANSFER-01",
            Self::TransferToSelf => "TX-TRANSFER-02",
            Self::HopsExhausted => "TX-TRANSFER-03",
            Self::RecipientWithoutIdentity => "TX-TRANSFER-04",
            Self::ReturnTransfer => "TX-TRANSFER-05",
            Self::RecipientBalanceCap => "TX-TRANSFER-06",
            Self::RecipientInboundCap => "TX-TRANSFER-07",
            Self::IdentityExists => "TX-ID-01",
            Self::CommitmentUsed => "TX-ID-02",
            Self::CommitmentOutOfField => "TX-ID-03",
            Self::AuthorCourseNotOwned => "TX-VA-01",
            Self::AuthorIdentityMissing => "TX-VA-02",
            Self::NullifierUsed => "TX-VA-03",
            Self::BadAuthorProof => "TX-VA-04",
            Self::Course(e) => e.rule_id(),
            Self::Overflow => "OVERFLOW",
        }
    }
}

impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {self:?}", self.rule_id())
    }
}

impl core::error::Error for TxError {}

impl From<Overflow> for TxError {
    fn from(_: Overflow) -> Self {
        Self::Overflow
    }
}

/// Validates and applies one user transaction, found at `position` in its block, to `overlay`.
pub(crate) fn apply_user_tx(
    overlay: &mut Overlay<'_>,
    tx: &Tx,
    position: u32,
    height: Height,
    chain_id: &Hash,
) -> Result<(), TxError> {
    if tx.encode().len() > MAX_TX_SIZE {
        return Err(TxError::TooLarge);
    }
    // BLK-09 already guarantees system transactions never reach here.
    let (Some(sender), Some(nonce), Some(signature)) = (tx.sender(), tx.nonce(), tx.signature())
    else {
        return Err(TxError::BadSignature);
    };
    let message = tx.sig_msg(chain_id).ok_or(TxError::BadSignature)?;
    if !verify_signature(&sender.0, message.as_bytes(), &signature.0) {
        return Err(TxError::BadSignature);
    }
    let expected = overlay.nonce(&sender);
    if nonce != expected {
        return Err(TxError::BadNonce {
            expected,
            found: nonce,
        });
    }

    let txid = tx.txid();
    match tx {
        Tx::Transfer(t) => apply_transfer(overlay, t, txid, height)?,
        Tx::RegisterIdentity(t) => apply_register_identity(overlay, t, txid, height)?,
        Tx::VerifyAuthor(t) => apply_verify_author(overlay, t, txid, height)?,
        Tx::BeginCourse(t) => apply_begin(overlay, t, txid, height)?,
        Tx::AppendFiles(t) => apply_append(overlay, t, height)?,
        Tx::AddLock(t) => apply_add_lock(overlay, t, txid, height)?,
        Tx::FinalizeCourse(t) => apply_finalize(overlay, t, txid, height, position)?,
        Tx::CancelCourse(t) => apply_cancel(overlay, t, txid, height)?,
        Tx::Coinbase(_) | Tx::Expire(_) | Tx::AutoCancel(_) => {}
    }
    overlay.set_nonce(sender, nonce.checked_add(1).ok_or(TxError::Overflow)?);
    Ok(())
}

/// Attributes of the consumed credit that outputs inherit (`spec/03-transactions.md` §4).
pub(crate) struct Consumed {
    pub(crate) hops: u8,
    pub(crate) renewals: u8,
}

/// TX-IN-01 – TX-IN-05: removes `inputs` from the overlay, takes exactly `required`, and returns
/// any change to the sender as output `(txid, change_index)`.
pub(crate) fn consume_inputs(
    overlay: &mut Overlay<'_>,
    sender: PubKey,
    inputs: &[OutPoint],
    required: Credit,
    height: Height,
    txid: Hash,
    change_index: u32,
) -> Result<Consumed, TxError> {
    if inputs.is_empty() {
        return Err(TxError::NoInputs);
    }
    let mut seen = BTreeSet::new();
    if !inputs.iter().all(|o| seen.insert(*o)) {
        return Err(TxError::DuplicateInput);
    }

    let mut utxos = Vec::with_capacity(inputs.len());
    for outpoint in inputs {
        let utxo = overlay
            .utxo(outpoint)
            .ok_or(TxError::MissingInput(*outpoint))?;
        if utxo.owner != sender {
            return Err(TxError::NotOwner(*outpoint));
        }
        if !utxo.is_spendable_at(height) {
            return Err(TxError::NotSpendable(*outpoint));
        }
        utxos.push((*outpoint, utxo));
    }

    let ordered = utxos.windows(2).all(
        |w| matches!(w, [(oa, a), (ob, b)] if (a.expire_height, *oa) < (b.expire_height, *ob)),
    );
    if !ordered {
        return Err(TxError::InputOrder);
    }

    let Some(((_, last), rest)) = utxos.split_last() else {
        return Err(TxError::NoInputs);
    };
    let before_last = rest
        .iter()
        .try_fold(Credit::ZERO, |acc, (_, u)| acc.checked_add(u.amount))
        .ok_or(TxError::Overflow)?;
    let total = before_last
        .checked_add(last.amount)
        .ok_or(TxError::Overflow)?;
    if !(before_last < required && required <= total) {
        return Err(TxError::InputsNotExact);
    }

    for (outpoint, _) in &utxos {
        overlay.remove(*outpoint)?;
    }
    let change = total.checked_sub(required).ok_or(TxError::Overflow)?;
    if change > Credit::ZERO {
        overlay.create(
            OutPoint {
                txid,
                index: change_index,
            },
            Utxo {
                amount: change,
                ..*last
            },
        )?;
    }
    Ok(Consumed {
        hops: utxos.iter().map(|(_, u)| u.hops).max().unwrap_or(0),
        renewals: utxos.iter().map(|(_, u)| u.renewals).max().unwrap_or(0),
    })
}

pub(crate) fn burn(overlay: &mut Overlay<'_>, amount: Credit) -> Result<(), TxError> {
    overlay.totals.burned = overlay
        .totals
        .burned
        .checked_add(amount)
        .ok_or(TxError::Overflow)?;
    Ok(())
}

/// `loss = max(TRANSFER_MIN_LOSS, ⌊amount × TRANSFER_LOSS_PERCENT / 100⌋)` (`spec/04-credit.md` §4).
#[must_use]
pub fn transfer_loss(amount: Credit) -> Option<Credit> {
    let proportional = amount
        .0
        .checked_mul(TRANSFER_LOSS_PERCENT)?
        .checked_div(100)?;
    Some(TRANSFER_MIN_LOSS.max(Credit(proportional)))
}

fn apply_transfer(
    overlay: &mut Overlay<'_>,
    tx: &UserTx<Transfer>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let Transfer {
        inputs,
        recipient,
        amount,
    } = &tx.body;
    if *amount < Credit(2) {
        return Err(TxError::AmountTooSmall);
    }
    if *recipient == sender {
        return Err(TxError::TransferToSelf);
    }
    if overlay.identity(recipient).is_none() {
        return Err(TxError::RecipientWithoutIdentity);
    }
    if overlay.transferred_since(
        *recipient,
        sender,
        window_start(height, NO_RETURN_WINDOW),
        height,
    ) {
        return Err(TxError::ReturnTransfer);
    }

    let consumed = consume_inputs(overlay, sender, inputs.as_slice(), *amount, height, txid, 1)?;
    if consumed.hops >= MAX_HOPS {
        return Err(TxError::HopsExhausted);
    }

    let loss = transfer_loss(*amount).ok_or(TxError::Overflow)?;
    let received = amount.checked_sub(loss).ok_or(TxError::Overflow)?;
    let new_balance = overlay
        .balance(recipient)
        .checked_add(received)
        .ok_or(TxError::Overflow)?;
    if new_balance > BALANCE_CAP {
        return Err(TxError::RecipientBalanceCap);
    }
    let inbound = overlay
        .inbound_since(*recipient, window_start(height, INBOUND_WINDOW), height)
        .and_then(|v| v.checked_add(received))
        .ok_or(TxError::Overflow)?;
    if inbound > INBOUND_CAP {
        return Err(TxError::RecipientInboundCap);
    }

    let expire_height = height.checked_add(EXPIRY_BLOCKS).ok_or(TxError::Overflow)?;
    overlay.create(
        OutPoint { txid, index: 0 },
        Utxo {
            owner: *recipient,
            amount: received,
            source: Source::Received,
            acquired_height: height,
            spendable_from: height,
            expire_height,
            hops: consumed.hops.checked_add(1).ok_or(TxError::Overflow)?,
            renewals: consumed.renewals,
        },
    )?;
    burn(overlay, loss)?;
    overlay.add_transfer(TransferEntry {
        from: sender,
        to: *recipient,
        received,
    });
    Ok(())
}

/// Whether `bytes`, read as a little-endian integer, is below `CIRCUIT_FIELD_MODULUS` (TX-ID-03).
#[must_use]
pub fn is_canonical_field_element(bytes: &[u8; 32]) -> bool {
    let mut be = *bytes;
    be.reverse();
    U256::from_be_bytes(be) < CIRCUIT_FIELD_MODULUS
}

fn apply_register_identity(
    overlay: &mut Overlay<'_>,
    tx: &UserTx<RegisterIdentity>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let commitment = tx.body.commitment;
    if overlay.identity(&sender).is_some() {
        return Err(TxError::IdentityExists);
    }
    if overlay.commitment_used(&commitment) {
        return Err(TxError::CommitmentUsed);
    }
    if !is_canonical_field_element(commitment.as_bytes()) {
        return Err(TxError::CommitmentOutOfField);
    }
    consume_inputs(
        overlay,
        sender,
        tx.body.inputs.as_slice(),
        COST_IDENTITY,
        height,
        txid,
        0,
    )?;
    burn(overlay, COST_IDENTITY)?;
    overlay.add_identity(sender, commitment);
    Ok(())
}

fn apply_verify_author(
    overlay: &mut Overlay<'_>,
    tx: &UserTx<VerifyAuthor>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let course_id = overlay
        .courses
        .owner_roots
        .get(&(sender, tx.body.course_root))
        .copied();
    if course_id.is_none_or(|id| overlay.courses.courses.get(&id).is_none()) {
        return Err(TxError::AuthorCourseNotOwned);
    }
    let commitment = overlay
        .identity(&sender)
        .ok_or(TxError::AuthorIdentityMissing)?;
    if overlay.nullifier_used(&tx.body.nullifier) {
        return Err(TxError::NullifierUsed);
    }
    verify_author_consensus(
        &commitment,
        &tx.body.challenge,
        &tx.body.course_root,
        &tx.body.nullifier,
        tx.body.proof.as_slice(),
    )
    .map_err(|_| TxError::BadAuthorProof)?;
    consume_inputs(
        overlay,
        sender,
        tx.body.inputs.as_slice(),
        COST_VERIFY_AUTHOR,
        height,
        txid,
        0,
    )?;
    burn(overlay, COST_VERIFY_AUTHOR)?;
    overlay.add_author_verification(AuthorVerification {
        sender,
        course_root: tx.body.course_root,
        challenge: tx.body.challenge,
        nullifier: tx.body.nullifier,
        height,
    });
    Ok(())
}
