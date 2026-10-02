//! Block store round-trip and crash-recovery tests.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    reason = "test code: any failure should abort the test"
)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use yuanlai_codec::BoundedBytes;
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{Chain, meets_target};
use yuanlai_storage::{BlockStore, StoreError};
use yuanlai_types::{Block, Network, PubKey, Timestamp};

fn temp_path() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("yuanlai-storage-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("blocks.dat")
}

fn sample_blocks(n: usize) -> Vec<Block> {
    let network = Network::Regtest;
    let mut chain = Chain::new(network.params(), &genesis_block(network).unwrap()).unwrap();
    let miner = PubKey(ed25519_key());
    let mut out = Vec::new();
    for _ in 0..n {
        let ts = Timestamp(
            chain
                .min_next_timestamp()
                .0
                .max(chain.tip().timestamp.0 + 60),
        );
        let mut block = chain.template(miner, BoundedBytes::empty(), ts).unwrap();
        while !meets_target(&block.header) {
            block.header.nonce += 1;
        }
        chain.connect(&block).unwrap();
        out.push(block);
    }
    out
}

/// RFC 8032 test vector 1 public key.
fn ed25519_key() -> [u8; 32] {
    [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ]
}

#[test]
fn roundtrip_preserves_order() {
    let path = temp_path();
    let blocks = sample_blocks(5);
    {
        let mut opened = BlockStore::open(&path).unwrap();
        assert!(opened.blocks.is_empty());
        for b in &blocks {
            opened.store.append(b).unwrap();
        }
    }
    let reopened = BlockStore::open(&path).unwrap();
    assert_eq!(reopened.blocks, blocks);
    assert_eq!(reopened.truncated_bytes, 0);
}

#[test]
fn replace_all_atomically_replaces_the_active_chain_records() {
    let path = temp_path();
    let old_blocks = sample_blocks(4);
    let replacement_blocks = sample_blocks(2);
    let mut opened = BlockStore::open(&path).unwrap();
    for block in &old_blocks {
        opened.store.append(block).unwrap();
    }

    opened.store.replace_all(&replacement_blocks).unwrap();
    drop(opened);

    assert_eq!(BlockStore::open(&path).unwrap().blocks, replacement_blocks);
}

#[test]
fn torn_tail_is_truncated_and_appends_continue() {
    let path = temp_path();
    let blocks = sample_blocks(3);
    {
        let mut opened = BlockStore::open(&path).unwrap();
        for b in &blocks[..2] {
            opened.store.append(b).unwrap();
        }
    }
    let good_len = std::fs::metadata(&path).unwrap().len();
    // Simulate a crash in the middle of writing the third record.
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"YLBK\x40\x00\x00\x00partial")
        .unwrap();

    let mut opened = BlockStore::open(&path).unwrap();
    assert_eq!(opened.blocks, blocks[..2]);
    assert_eq!(opened.truncated_bytes, 15);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), good_len);

    opened.store.append(&blocks[2]).unwrap();
    drop(opened);
    assert_eq!(BlockStore::open(&path).unwrap().blocks, blocks);
}

#[test]
fn torn_inside_header_is_truncated() {
    let path = temp_path();
    std::fs::write(&path, b"YLB").unwrap();
    let opened = BlockStore::open(&path).unwrap();
    assert!(opened.blocks.is_empty());
    assert_eq!(opened.truncated_bytes, 3);
}

#[test]
fn corrupted_complete_record_is_an_error_not_silently_dropped() {
    let path = temp_path();
    let blocks = sample_blocks(2);
    {
        let mut opened = BlockStore::open(&path).unwrap();
        for b in &blocks {
            opened.store.append(b).unwrap();
        }
    }
    let mut bytes = std::fs::read(&path).unwrap();
    // Flip one payload byte of the first record.
    bytes[20] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
    match BlockStore::open(&path) {
        Err(StoreError::Corrupt { offset: 0, reason }) => assert_eq!(reason, "checksum mismatch"),
        other => panic!("expected corruption error, got {other:?}"),
    }

    std::fs::write(&path, b"XXXX\x00\x00\x00\x00\x00\x00\x00\x00").unwrap();
    assert!(matches!(
        BlockStore::open(&path),
        Err(StoreError::Corrupt {
            reason: "bad magic",
            ..
        })
    ));
}

#[test]
fn oversized_length_is_rejected_before_allocation() {
    let path = temp_path();
    let mut bytes = b"YLBK".to_vec();
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        BlockStore::open(&path),
        Err(StoreError::Corrupt {
            reason: "record longer than MAX_BLOCK_SIZE",
            ..
        })
    ));
}
