use yuanlai_codec::{BoundedVec, Decode, DecodeError, Encode, Reader};
use yuanlai_crypto::{Hash, Tag, TaggedHasher, merkle_root};

use crate::params::MAX_BLOCK_TXS;
use crate::{Height, Target, Timestamp, Tx};

/// Block header (`spec/02-block.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    /// Header version (BLK-02).
    pub version: u32,
    /// Network identifier (BLK-03).
    pub chain_id: Hash,
    /// Height; the genesis block is 0.
    pub height: Height,
    /// Parent block ID; all zeros for genesis.
    pub prev_block: Hash,
    /// Merkle root of transaction IDs.
    pub tx_root: Hash,
    /// Block time in Unix seconds.
    pub timestamp: Timestamp,
    /// Proof-of-work target.
    pub target: Target,
    /// Proof-of-work nonce.
    pub nonce: u64,
}

impl BlockHeader {
    /// Encoded header size in bytes.
    pub const ENCODED_SIZE: usize = 156;

    /// `TH("YuanLai/block/v1", header)`.
    #[must_use]
    pub fn id(&self) -> Hash {
        let mut hasher = TaggedHasher::new(Tag::Block);
        hasher.update(&self.encode());
        hasher.finalize()
    }
}

impl Encode for BlockHeader {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.version.encode_to(out);
        self.chain_id.encode_to(out);
        self.height.encode_to(out);
        self.prev_block.encode_to(out);
        self.tx_root.encode_to(out);
        self.timestamp.encode_to(out);
        self.target.encode_to(out);
        self.nonce.encode_to(out);
    }
}

impl Decode for BlockHeader {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            version: u32::decode_from(reader)?,
            chain_id: Hash::decode_from(reader)?,
            height: Height::decode_from(reader)?,
            prev_block: Hash::decode_from(reader)?,
            tx_root: Hash::decode_from(reader)?,
            timestamp: Timestamp::decode_from(reader)?,
            target: Target::decode_from(reader)?,
            nonce: u64::decode_from(reader)?,
        })
    }
}

/// A block (`spec/02-block.md` §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The header.
    pub header: BlockHeader,
    /// Transactions in block order.
    pub txs: BoundedVec<Tx, MAX_BLOCK_TXS>,
}

impl Block {
    /// `MTH([txid₀, txid₁, …])`, the value `header.tx_root` must equal (BLK-10).
    #[must_use]
    pub fn compute_tx_root(&self) -> Hash {
        let txids: Vec<Hash> = self.txs.iter().map(Tx::txid).collect();
        merkle_root(&txids)
    }
}

impl Encode for Block {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.header.encode_to(out);
        self.txs.encode_to(out);
    }
}

impl Decode for Block {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            header: BlockHeader::decode_from(reader)?,
            txs: BoundedVec::decode_from(reader)?,
        })
    }
}
