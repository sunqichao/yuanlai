use core::fmt;

use yuanlai_codec::{BoundedBytes, BoundedVec, Decode, DecodeError, Encode, Reader};
use yuanlai_crypto::{Hash, Tag, TaggedHasher, tagged_hash};

use crate::course::{AddLock, AppendFiles, BeginCourse, SessionRef};
use crate::params::{MAX_COINBASE_EXTRA, MAX_INPUTS, MAX_PROOF_SIZE};
use crate::{Credit, Height, OutPoint, PubKey};

const TAG_COINBASE: u8 = 0x00;
const TAG_EXPIRE: u8 = 0x01;
const TAG_AUTO_CANCEL: u8 = 0x02;
const TAG_TRANSFER: u8 = 0x10;
const TAG_REGISTER_IDENTITY: u8 = 0x11;
const TAG_VERIFY_AUTHOR: u8 = 0x30;
const TAG_BEGIN_COURSE: u8 = 0x20;
const TAG_APPEND_FILES: u8 = 0x21;
const TAG_ADD_LOCK: u8 = 0x22;
const TAG_FINALIZE_COURSE: u8 = 0x23;
const TAG_CANCEL_COURSE: u8 = 0x24;

/// Block reward transaction (`spec/03-transactions.md` §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coinbase {
    /// Must equal the containing block's height (TX-CB-01).
    pub height: Height,
    /// Recipient of the reward (delegated mining).
    pub beneficiary: PubKey,
    /// Free-form miner data; the genesis block stores a public reference here.
    pub extra: BoundedBytes<MAX_COINBASE_EXTRA>,
}

/// Commitment to deterministically derived records, used by `Expire` and `AutoCancel`
/// (`spec/03-transactions.md` §6.2–6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordCommitment {
    /// Must equal the containing block's height.
    pub height: Height,
    /// Number of records.
    pub count: u32,
    /// `MTH` over the encoded records.
    pub records_root: Hash,
}

/// An Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl Signature {
    /// Placeholder used while computing the signing message.
    pub const EMPTY: Self = Self([0; 64]);
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Signature(")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

impl Encode for Signature {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.0.encode_to(out);
    }
}

impl Decode for Signature {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        reader.take_array().map(Self)
    }
}

/// Envelope of every user transaction (`spec/03-transactions.md` §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTx<B> {
    /// Signer and owner of all inputs.
    pub sender: PubKey,
    /// Must equal the sender's account sequence number (TX-NONCE-01).
    pub nonce: u64,
    /// Type-specific fields.
    pub body: B,
    /// Signature over `sig_msg`.
    pub signature: Signature,
}

/// `Transfer` body (`spec/04-credit.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// Inputs in consumption order.
    pub inputs: BoundedVec<OutPoint, MAX_INPUTS>,
    /// Receiving address.
    pub recipient: PubKey,
    /// Gross amount taken from the sender, including the burned loss.
    pub amount: Credit,
}

impl Encode for Transfer {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.inputs.encode_to(out);
        self.recipient.encode_to(out);
        self.amount.encode_to(out);
    }
}

impl Decode for Transfer {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            inputs: BoundedVec::decode_from(reader)?,
            recipient: PubKey::decode_from(reader)?,
            amount: Credit::decode_from(reader)?,
        })
    }
}

/// `RegisterIdentity` body (`spec/03-transactions.md` §6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterIdentity {
    /// Inputs in consumption order.
    pub inputs: BoundedVec<OutPoint, MAX_INPUTS>,
    /// Identity commitment: a circuit field element, little-endian.
    pub commitment: Hash,
}

impl Encode for RegisterIdentity {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.inputs.encode_to(out);
        self.commitment.encode_to(out);
    }
}

impl Decode for RegisterIdentity {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            inputs: BoundedVec::decode_from(reader)?,
            commitment: Hash::decode_from(reader)?,
        })
    }
}

/// `VerifyAuthor` body (`spec/03-transactions.md` §6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyAuthor {
    /// Inputs in consumption order.
    pub inputs: BoundedVec<OutPoint, MAX_INPUTS>,
    /// Registered course root being verified.
    pub course_root: Hash,
    /// Fresh challenge issued by the verifier.
    pub challenge: Hash,
    /// Challenge- and course-bound, one-time nullifier.
    pub nullifier: Hash,
    /// Raw Halo2 proof transcript bytes.
    pub proof: BoundedBytes<MAX_PROOF_SIZE>,
}

impl Encode for VerifyAuthor {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.inputs.encode_to(out);
        self.course_root.encode_to(out);
        self.challenge.encode_to(out);
        self.nullifier.encode_to(out);
        self.proof.encode_to(out);
    }
}

impl Decode for VerifyAuthor {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            inputs: BoundedVec::decode_from(reader)?,
            course_root: Hash::decode_from(reader)?,
            challenge: Hash::decode_from(reader)?,
            nullifier: Hash::decode_from(reader)?,
            proof: BoundedBytes::decode_from(reader)?,
        })
    }
}

/// A transaction (`spec/03-transactions.md` §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tx {
    /// Tag `0x00`, system.
    Coinbase(Coinbase),
    /// Tag `0x01`, system.
    Expire(RecordCommitment),
    /// Tag `0x02`, system.
    AutoCancel(RecordCommitment),
    /// Tag `0x10`.
    Transfer(UserTx<Transfer>),
    /// Tag `0x11`.
    RegisterIdentity(UserTx<RegisterIdentity>),
    /// Tag `0x12`.
    VerifyAuthor(UserTx<VerifyAuthor>),
    /// Tag `0x20`.
    BeginCourse(UserTx<BeginCourse>),
    /// Tag `0x21`.
    AppendFiles(UserTx<AppendFiles>),
    /// Tag `0x22`.
    AddLock(UserTx<AddLock>),
    /// Tag `0x23`.
    FinalizeCourse(UserTx<SessionRef>),
    /// Tag `0x24`.
    CancelCourse(UserTx<SessionRef>),
}

/// Evaluates `$user` with `$t` bound to the `UserTx<_>` of any user variant, or `$system` otherwise.
macro_rules! with_user_tx {
    ($tx:expr, $t:ident => $user:expr, _ => $system:expr) => {
        match $tx {
            Tx::Transfer($t) => $user,
            Tx::RegisterIdentity($t) => $user,
            Tx::VerifyAuthor($t) => $user,
            Tx::BeginCourse($t) => $user,
            Tx::AppendFiles($t) => $user,
            Tx::AddLock($t) => $user,
            Tx::FinalizeCourse($t) => $user,
            Tx::CancelCourse($t) => $user,
            Tx::Coinbase(_) | Tx::Expire(_) | Tx::AutoCancel(_) => $system,
        }
    };
}

impl Tx {
    /// `TH("YuanLai/tx/v1", full encoding)`.
    #[must_use]
    pub fn txid(&self) -> Hash {
        tagged_hash(Tag::Tx, &self.encode())
    }

    /// The encoding tag.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::Coinbase(_) => TAG_COINBASE,
            Self::Expire(_) => TAG_EXPIRE,
            Self::AutoCancel(_) => TAG_AUTO_CANCEL,
            Self::Transfer(_) => TAG_TRANSFER,
            Self::RegisterIdentity(_) => TAG_REGISTER_IDENTITY,
            Self::VerifyAuthor(_) => TAG_VERIFY_AUTHOR,
            Self::BeginCourse(_) => TAG_BEGIN_COURSE,
            Self::AppendFiles(_) => TAG_APPEND_FILES,
            Self::AddLock(_) => TAG_ADD_LOCK,
            Self::FinalizeCourse(_) => TAG_FINALIZE_COURSE,
            Self::CancelCourse(_) => TAG_CANCEL_COURSE,
        }
    }

    /// Whether this is a system transaction (no signature).
    #[must_use]
    pub const fn is_system(&self) -> bool {
        matches!(
            self,
            Self::Coinbase(_) | Self::Expire(_) | Self::AutoCancel(_)
        )
    }

    /// Sender of a user transaction.
    #[must_use]
    pub const fn sender(&self) -> Option<PubKey> {
        with_user_tx!(self, t => Some(t.sender), _ => None)
    }

    /// Nonce of a user transaction.
    #[must_use]
    pub const fn nonce(&self) -> Option<u64> {
        with_user_tx!(self, t => Some(t.nonce), _ => None)
    }

    /// Signature of a user transaction.
    #[must_use]
    pub const fn signature(&self) -> Option<Signature> {
        with_user_tx!(self, t => Some(t.signature), _ => None)
    }

    /// Replaces the signature of a user transaction. Returns `false` for system transactions.
    pub const fn set_signature(&mut self, signature: Signature) -> bool {
        with_user_tx!(self, t => { t.signature = signature; true }, _ => false)
    }

    /// `sig_msg = TH("YuanLai/tx-sig/v1", chain_id ‖ encoding without signature)`, or `None` for
    /// system transactions.
    #[must_use]
    pub fn sig_msg(&self, chain_id: &Hash) -> Option<Hash> {
        let mut unsigned = Vec::new();
        let tag = self.tag();
        with_user_tx!(self, t => encode_user_unsigned(tag, t, &mut unsigned), _ => return None);
        let mut hasher = TaggedHasher::new(Tag::TxSig);
        hasher.update(chain_id.as_bytes());
        hasher.update(&unsigned);
        Some(hasher.finalize())
    }
}

fn encode_commitment(tag: u8, c: &RecordCommitment, out: &mut Vec<u8>) {
    out.push(tag);
    c.height.encode_to(out);
    c.count.encode_to(out);
    c.records_root.encode_to(out);
}

fn decode_commitment(reader: &mut Reader<'_>) -> Result<RecordCommitment, DecodeError> {
    Ok(RecordCommitment {
        height: Height::decode_from(reader)?,
        count: u32::decode_from(reader)?,
        records_root: Hash::decode_from(reader)?,
    })
}

fn encode_user_unsigned<B: Encode>(tag: u8, tx: &UserTx<B>, out: &mut Vec<u8>) {
    out.push(tag);
    tx.sender.encode_to(out);
    tx.nonce.encode_to(out);
    tx.body.encode_to(out);
}

fn decode_user<B: Decode>(reader: &mut Reader<'_>) -> Result<UserTx<B>, DecodeError> {
    Ok(UserTx {
        sender: PubKey::decode_from(reader)?,
        nonce: u64::decode_from(reader)?,
        body: B::decode_from(reader)?,
        signature: Signature::decode_from(reader)?,
    })
}

impl Encode for Tx {
    fn encode_to(&self, out: &mut Vec<u8>) {
        let tag = self.tag();
        match self {
            Self::Coinbase(cb) => {
                out.push(tag);
                cb.height.encode_to(out);
                cb.beneficiary.encode_to(out);
                cb.extra.encode_to(out);
            }
            Self::Expire(c) | Self::AutoCancel(c) => encode_commitment(tag, c, out),
            user => with_user_tx!(user, t => {
                encode_user_unsigned(tag, t, out);
                t.signature.encode_to(out);
            }, _ => {}),
        }
    }
}

impl Decode for Tx {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::decode_from(reader)? {
            TAG_COINBASE => Ok(Self::Coinbase(Coinbase {
                height: Height::decode_from(reader)?,
                beneficiary: PubKey::decode_from(reader)?,
                extra: BoundedBytes::decode_from(reader)?,
            })),
            TAG_EXPIRE => decode_commitment(reader).map(Self::Expire),
            TAG_AUTO_CANCEL => decode_commitment(reader).map(Self::AutoCancel),
            TAG_TRANSFER => decode_user(reader).map(Self::Transfer),
            TAG_REGISTER_IDENTITY => decode_user(reader).map(Self::RegisterIdentity),
            TAG_VERIFY_AUTHOR => decode_user(reader).map(Self::VerifyAuthor),
            TAG_BEGIN_COURSE => decode_user(reader).map(Self::BeginCourse),
            TAG_APPEND_FILES => decode_user(reader).map(Self::AppendFiles),
            TAG_ADD_LOCK => decode_user(reader).map(Self::AddLock),
            TAG_FINALIZE_COURSE => decode_user(reader).map(Self::FinalizeCourse),
            TAG_CANCEL_COURSE => decode_user(reader).map(Self::CancelCourse),
            other => Err(DecodeError::UnknownTag(other)),
        }
    }
}
