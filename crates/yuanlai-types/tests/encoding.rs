//! Encoding tests for headers, blocks and transactions.

#![allow(
    clippy::unwrap_used,
    reason = "test code: any failure should abort the test"
)]

use yuanlai_codec::{BoundedBytes, BoundedVec, DecodeError, Encode, decode_exact};
use yuanlai_types::params::{HEADER_VERSION, POW_LIMIT};
use yuanlai_types::{
    Block, BlockHeader, Coinbase, Hash, Height, Network, OutPoint, PubKey, Signature, Target,
    Timestamp, Tx, U256, UserTx, VerifyAuthor,
};

fn sample_header() -> BlockHeader {
    BlockHeader {
        version: HEADER_VERSION,
        chain_id: Network::Devnet.chain_id(),
        height: Height(7),
        prev_block: Hash([0x11; 32]),
        tx_root: Hash([0x22; 32]),
        timestamp: Timestamp(1_790_000_000),
        target: Target(POW_LIMIT),
        nonce: 0x0102_0304_0506_0708,
    }
}

#[test]
fn header_has_fixed_size_and_field_layout() {
    let header = sample_header();
    let bytes = header.encode();
    assert_eq!(bytes.len(), BlockHeader::ENCODED_SIZE);
    assert_eq!(bytes.get(0..4), Some(&[1, 0, 0, 0][..]));
    assert_eq!(bytes.get(36..44), Some(&7_u64.to_le_bytes()[..]));
    // Target is the only big-endian field: POW_LIMIT = 2^236 - 1 starts with 20 zero bits.
    assert_eq!(bytes.get(116..119), Some(&[0x00, 0x00, 0x0F][..]));
    assert_eq!(
        bytes.get(148..156),
        Some(&0x0102_0304_0506_0708_u64.to_le_bytes()[..])
    );
}

#[test]
fn header_roundtrip_and_strictness() {
    let header = sample_header();
    let bytes = header.encode();
    assert_eq!(decode_exact::<BlockHeader>(&bytes), Ok(header));

    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(
        decode_exact::<BlockHeader>(&long),
        Err(DecodeError::TrailingBytes)
    );
    assert_eq!(
        decode_exact::<BlockHeader>(bytes.get(..155).unwrap()),
        Err(DecodeError::UnexpectedEnd)
    );
}

#[test]
fn header_id_depends_on_every_field() {
    let base = sample_header();
    let mut variants = vec![base];
    variants.push(BlockHeader { version: 2, ..base });
    variants.push(BlockHeader {
        chain_id: Network::Mainnet.chain_id(),
        ..base
    });
    variants.push(BlockHeader {
        height: Height(8),
        ..base
    });
    variants.push(BlockHeader {
        prev_block: Hash([0x12; 32]),
        ..base
    });
    variants.push(BlockHeader {
        tx_root: Hash([0x23; 32]),
        ..base
    });
    variants.push(BlockHeader {
        timestamp: Timestamp(1_790_000_001),
        ..base
    });
    variants.push(BlockHeader {
        target: Target(U256::ONE),
        ..base
    });
    variants.push(BlockHeader { nonce: 0, ..base });
    let mut ids: Vec<Hash> = variants.iter().map(BlockHeader::id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), variants.len());
}

#[test]
fn chain_ids_are_distinct() {
    let ids = [Network::Mainnet, Network::Testnet, Network::Devnet].map(Network::chain_id);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_ne!(ids[0], ids[2]);
}

#[test]
fn coinbase_roundtrip_and_unknown_tag() {
    let tx = Tx::Coinbase(Coinbase {
        height: Height(3),
        beneficiary: PubKey([0xAB; 32]),
        extra: BoundedBytes::new(b"hello".to_vec()).unwrap(),
    });
    let bytes = tx.encode();
    assert_eq!(bytes.first(), Some(&0x00));
    assert_eq!(decode_exact::<Tx>(&bytes), Ok(tx));

    let mut unknown = bytes;
    if let Some(tag) = unknown.first_mut() {
        *tag = 0xFF;
    }
    assert_eq!(
        decode_exact::<Tx>(&unknown),
        Err(DecodeError::UnknownTag(0xFF))
    );
}

#[test]
fn coinbase_extra_is_bounded() {
    assert!(BoundedBytes::<64>::new(vec![0; 64]).is_ok());
    assert!(BoundedBytes::<64>::new(vec![0; 65]).is_err());
}

#[test]
fn verify_author_roundtrip_and_proof_bound() {
    let tx = Tx::VerifyAuthor(UserTx {
        sender: PubKey([0x31; 32]),
        nonce: 7,
        body: VerifyAuthor {
            inputs: BoundedVec::new(vec![OutPoint {
                txid: Hash([0x41; 32]),
                index: 2,
            }])
            .unwrap(),
            course_root: Hash([0x51; 32]),
            challenge: Hash([0x61; 32]),
            nullifier: Hash([0x71; 32]),
            proof: BoundedBytes::new(vec![0x81, 0x82, 0x83]).unwrap(),
        },
        signature: Signature([0x91; 64]),
    });
    let encoded = tx.encode();
    assert_eq!(encoded.first(), Some(&0x30));
    assert_eq!(decode_exact::<Tx>(&encoded), Ok(tx));
    assert!(BoundedBytes::<8192>::new(vec![0; 8193]).is_err());
}

#[test]
fn block_tx_root_commits_to_order() {
    let make = |h| {
        Tx::Coinbase(Coinbase {
            height: Height(h),
            beneficiary: PubKey([0; 32]),
            extra: BoundedBytes::empty(),
        })
    };
    let block_ab = Block {
        header: sample_header(),
        txs: BoundedVec::new(vec![make(1), make(2)]).unwrap(),
    };
    let block_ba = Block {
        header: sample_header(),
        txs: BoundedVec::new(vec![make(2), make(1)]).unwrap(),
    };
    assert_ne!(block_ab.compute_tx_root(), block_ba.compute_tx_root());
    assert_eq!(decode_exact::<Block>(&block_ab.encode()), Ok(block_ab));
}
