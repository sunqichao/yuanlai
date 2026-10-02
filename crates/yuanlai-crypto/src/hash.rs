use core::fmt;

use sha2::{Digest, Sha256};
use yuanlai_codec::{Decode, DecodeError, Encode, Reader};

/// A 32-byte hash value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// The all-zero hash, used as `prev_block` of the genesis block.
    pub const ZERO: Self = Self([0; 32]);

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Encode for Hash {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.0.encode_to(out);
    }
}

impl Decode for Hash {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        reader.take_array().map(Self)
    }
}

/// Every tag used by the protocol (`spec/01-serialization.md` §4). No two purposes share a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// `YuanLai/chain-id/v1`
    ChainId,
    /// `YuanLai/block/v1`
    Block,
    /// `YuanLai/tx/v1`
    Tx,
    /// `YuanLai/tx-sig/v1`
    TxSig,
    /// `YuanLai/merkle-empty/v1`
    MerkleEmpty,
    /// `YuanLai/merkle-leaf/v1`
    MerkleLeaf,
    /// `YuanLai/merkle-node/v1`
    MerkleNode,
}

impl Tag {
    /// The ASCII tag string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChainId => "YuanLai/chain-id/v1",
            Self::Block => "YuanLai/block/v1",
            Self::Tx => "YuanLai/tx/v1",
            Self::TxSig => "YuanLai/tx-sig/v1",
            Self::MerkleEmpty => "YuanLai/merkle-empty/v1",
            Self::MerkleLeaf => "YuanLai/merkle-leaf/v1",
            Self::MerkleNode => "YuanLai/merkle-node/v1",
        }
    }
}

/// Plain SHA-256.
#[must_use]
pub fn sha256(data: &[u8]) -> Hash {
    Hash(Sha256::digest(data).into())
}

/// Incremental `TH(tag, m)`. Cloning reuses the precomputed tag prefix.
#[derive(Clone)]
pub struct TaggedHasher {
    inner: Sha256,
}

impl TaggedHasher {
    /// Starts `TH(tag, ·)` by absorbing `SHA256(tag) ‖ SHA256(tag)`.
    #[must_use]
    pub fn new(tag: Tag) -> Self {
        let tag_hash = Sha256::digest(tag.as_str().as_bytes());
        let mut inner = Sha256::new();
        inner.update(tag_hash);
        inner.update(tag_hash);
        Self { inner }
    }

    /// Absorbs message bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finishes the hash.
    #[must_use]
    pub fn finalize(self) -> Hash {
        Hash(self.inner.finalize().into())
    }
}

/// `TH(tag, m) = SHA256(SHA256(tag) ‖ SHA256(tag) ‖ m)`.
#[must_use]
pub fn tagged_hash(tag: Tag, data: &[u8]) -> Hash {
    let mut hasher = TaggedHasher::new(tag);
    hasher.update(data);
    hasher.finalize()
}
