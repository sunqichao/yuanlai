use yuanlai_codec::{Decode, DecodeError, Encode, Reader};
use yuanlai_crypto::Hash;

use crate::{Credit, Height, PubKey};

/// Reference to a transaction output. Ordered by `txid` bytes, then `index` numerically
/// (`spec/03-transactions.md` §2); the derived `Ord` implements exactly this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutPoint {
    /// Creating transaction.
    pub txid: Hash,
    /// Output index within that transaction.
    pub index: u32,
}

impl Encode for OutPoint {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.txid.encode_to(out);
        self.index.encode_to(out);
    }
}

impl Decode for OutPoint {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            txid: Hash::decode_from(reader)?,
            index: u32::decode_from(reader)?,
        })
    }
}

/// How a UTXO was obtained (`spec/04-credit.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// Tag 0: block reward.
    Mined,
    /// Tag 1: incoming transfer.
    Received,
    /// Tag 2: remainder released by a finalized course session.
    Released,
    /// Tag 3: restored by a cancelled or timed-out course session.
    Cancelled,
}

impl Source {
    /// The encoding tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Mined => 0,
            Self::Received => 1,
            Self::Released => 2,
            Self::Cancelled => 3,
        }
    }
}

impl Encode for Source {
    fn encode_to(&self, out: &mut Vec<u8>) {
        out.push(self.tag());
    }
}

impl Decode for Source {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::decode_from(reader)? {
            0 => Ok(Self::Mined),
            1 => Ok(Self::Received),
            2 => Ok(Self::Released),
            3 => Ok(Self::Cancelled),
            other => Err(DecodeError::UnknownTag(other)),
        }
    }
}

/// An unspent credit output (`spec/04-credit.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utxo {
    /// Owner.
    pub owner: PubKey,
    /// Amount; always positive.
    pub amount: Credit,
    /// How it was obtained.
    pub source: Source,
    /// Start of the current validity period.
    pub acquired_height: Height,
    /// First height at which it can be spent.
    pub spendable_from: Height,
    /// Height at which it is removed by `Expire`.
    pub expire_height: Height,
    /// Number of transfers so far.
    pub hops: u8,
    /// Number of validity renewals from ending course sessions.
    pub renewals: u8,
}

impl Utxo {
    /// Whether the UTXO can be spent in a block at `height` (`spec/04-credit.md` §2).
    #[must_use]
    pub fn is_spendable_at(&self, height: Height) -> bool {
        self.spendable_from <= height && height < self.expire_height
    }
}

/// Record of one expired UTXO, committed to by `Expire.records_root` (`spec/04-credit.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpireRecord {
    /// The expired output.
    pub outpoint: OutPoint,
    /// Its data at the time of expiry.
    pub utxo: Utxo,
}

impl Encode for ExpireRecord {
    fn encode_to(&self, out: &mut Vec<u8>) {
        let u = &self.utxo;
        self.outpoint.encode_to(out);
        u.owner.encode_to(out);
        u.amount.encode_to(out);
        u.source.encode_to(out);
        u.acquired_height.encode_to(out);
        u.expire_height.encode_to(out);
        u.hops.encode_to(out);
        u.renewals.encode_to(out);
    }
}
