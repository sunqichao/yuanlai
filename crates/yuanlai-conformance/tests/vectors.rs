//! Checks the Rust implementation against `spec/test-vectors/*.json`.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: any failure should abort the test"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use yuanlai_codec::{BoundedBytes, BoundedText, BoundedVec, DecodeError, Encode, decode_exact};
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{Ancestry, Chain, block_work, median_time_past, meets_target, next_target};
use yuanlai_crypto::{Tag, merkle_root, tagged_hash};
use yuanlai_types::params::RETARGET_INTERVAL;
use yuanlai_types::{Block, BlockHeader, Hash, Height, Network, Target, Timestamp, Tx, U256};

fn load(name: &str) -> Value {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "spec",
        "test-vectors",
        name,
    ]
    .iter()
    .collect();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn cases<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("missing array {key}"))
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn hash(v: &Value) -> Hash {
    Hash(bytes(v).try_into().unwrap())
}

fn u256(v: &Value) -> U256 {
    U256::from_be_bytes(bytes(v).try_into().unwrap())
}

fn num(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

const ALL_TAGS: [Tag; 7] = [
    Tag::ChainId,
    Tag::Block,
    Tag::Tx,
    Tag::TxSig,
    Tag::MerkleEmpty,
    Tag::MerkleLeaf,
    Tag::MerkleNode,
];

#[test]
fn tagged_hash_vectors() {
    let v = load("tagged-hash.json");
    let mut tags_seen = std::collections::BTreeSet::new();
    for c in cases(&v, "cases") {
        let name = c["tag"].as_str().unwrap();
        let tag = ALL_TAGS.into_iter().find(|t| t.as_str() == name).unwrap();
        tags_seen.insert(name);
        assert_eq!(
            tagged_hash(tag, &bytes(&c["message"])),
            hash(&c["hash"]),
            "{name}"
        );
    }
    assert_eq!(
        tags_seen.len(),
        ALL_TAGS.len(),
        "every tag must have vectors"
    );

    for c in cases(&v, "chain_ids") {
        let network = match c["network"].as_str().unwrap() {
            "mainnet" => Network::Mainnet,
            "testnet" => Network::Testnet,
            "devnet" => Network::Devnet,
            "regtest" => Network::Regtest,
            other => panic!("unknown network {other}"),
        };
        assert_eq!(network.name(), c["name"].as_str().unwrap());
        assert_eq!(network.chain_id(), hash(&c["chain_id"]));
    }
}

#[test]
fn merkle_vectors() {
    let v = load("merkle.json");
    for c in cases(&v, "cases") {
        let leaves: Vec<Vec<u8>> = c["leaves"].as_array().unwrap().iter().map(bytes).collect();
        assert_eq!(
            merkle_root(&leaves),
            hash(&c["root"]),
            "{} leaves",
            leaves.len()
        );
    }
}

#[test]
fn block_header_vectors() {
    let v = load("block-header.json");
    for c in cases(&v, "cases") {
        let description = c["description"].as_str().unwrap();
        let header = BlockHeader {
            version: u32::try_from(num(&c["version"])).unwrap(),
            chain_id: hash(&c["chain_id"]),
            height: Height(num(&c["height"])),
            prev_block: hash(&c["prev_block"]),
            tx_root: hash(&c["tx_root"]),
            timestamp: Timestamp(num(&c["timestamp"])),
            target: Target(u256(&c["target"])),
            nonce: num(&c["nonce"]),
        };
        let encoding = bytes(&c["encoding"]);
        assert_eq!(header.encode(), encoding, "{description}");
        assert_eq!(
            decode_exact::<BlockHeader>(&encoding),
            Ok(header),
            "{description}"
        );
        assert_eq!(header.id(), hash(&c["id"]), "{description}");
        assert_eq!(
            meets_target(&header),
            c["meets_target"].as_bool().unwrap(),
            "{description}"
        );
        assert_eq!(
            block_work(header.target),
            Some(u256(&c["work"])),
            "{description}"
        );
    }
    for c in cases(&v, "work") {
        assert_eq!(
            block_work(Target(u256(&c["target"]))),
            Some(u256(&c["work"]))
        );
    }
}

/// Exposes exactly the two timestamps a vector provides, so reading any other height fails.
struct VectorAncestry(BTreeMap<Height, Timestamp>);

impl Ancestry for VectorAncestry {
    fn timestamp_at(&self, height: Height) -> Option<Timestamp> {
        self.0.get(&height).copied()
    }
}

#[test]
fn difficulty_vectors() {
    let v = load("difficulty.json");
    for c in cases(&v, "cases") {
        let description = c["description"].as_str().unwrap();
        let h = num(&c["next_height"]);
        let mut known = BTreeMap::new();
        if let (Some(first), Some(last)) = (c["ts_first"].as_u64(), c["ts_last"].as_u64()) {
            if let Some(first_height) = h.checked_sub(RETARGET_INTERVAL + 1) {
                known.insert(Height(first_height), Timestamp(first));
            }
            known.insert(Height(h - 1), Timestamp(last));
        }
        let got = next_target(
            Height(h),
            Target(u256(&c["parent_target"])),
            u256(&c["pow_limit"]),
            &VectorAncestry(known),
        )
        .unwrap();
        assert_eq!(got, Target(u256(&c["expected"])), "{description}");
    }

    for c in cases(&v, "median_time_past") {
        let ts: Vec<Timestamp> = c["timestamps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| Timestamp(num(t)))
            .collect();
        assert_eq!(median_time_past(&ts), Some(Timestamp(num(&c["median"]))));
    }
}

fn decode_as(ty: &str, input: &[u8]) -> Result<(), DecodeError> {
    match ty {
        "u16" => decode_exact::<u16>(input).map(drop),
        "u64" => decode_exact::<u64>(input).map(drop),
        "bool" => decode_exact::<bool>(input).map(drop),
        "option_u8" => decode_exact::<Option<u8>>(input).map(drop),
        "varbytes_4" => decode_exact::<BoundedBytes<4>>(input).map(drop),
        "text_8" => decode_exact::<BoundedText<8>>(input).map(drop),
        "list_u16_3" => decode_exact::<BoundedVec<u16, 3>>(input).map(drop),
        "tx" => decode_exact::<Tx>(input).map(drop),
        other => panic!("unknown vector type {other}"),
    }
}

#[test]
fn codec_vectors() {
    let v = load("codec.json");
    for c in cases(&v, "cases") {
        let ty = c["type"].as_str().unwrap();
        let input = bytes(&c["hex"]);
        let got = decode_as(ty, &input).map_err(|e| e.rule_id());
        let expected = c["error"].as_str().map_or(Ok(()), Err);
        assert_eq!(got, expected, "{ty} {}", c["hex"]);
    }
}

#[test]
fn genesis_vectors() {
    let v = load("genesis.json");
    for c in cases(&v, "cases") {
        let network = match c["network"].as_str().unwrap() {
            "devnet" => Network::Devnet,
            "regtest" => Network::Regtest,
            other => panic!("unexpected network {other}"),
        };
        let expected = bytes(&c["block"]);
        let block = genesis_block(network).unwrap();
        assert_eq!(block.encode(), expected, "{network:?}");
        assert_eq!(decode_exact::<Block>(&expected), Ok(block.clone()));
        assert_eq!(block.header.id(), hash(&c["id"]));
        assert!(Chain::new(network.params(), &block).is_ok());
    }
}

fn network_named(name: &str) -> Network {
    match name {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        "devnet" => Network::Devnet,
        "regtest" => Network::Regtest,
        other => panic!("unknown network {other}"),
    }
}

#[test]
fn tx_vectors() {
    let v = load("tx.json");
    for c in cases(&v, "cases") {
        let label = format!("{} {}", c["type"], c["network"]);
        if c["type"] == "verify_author" {
            assert_eq!(
                c["proof_status"].as_str(),
                Some("synthetic wire-format payload; not a valid Halo2 proof")
            );
        }
        let encoding = bytes(&c["encoding"]);
        let tx = decode_exact::<Tx>(&encoding).unwrap();
        assert_eq!(tx.encode(), encoding, "{label}");
        assert_eq!(tx.txid(), hash(&c["txid"]), "{label}");

        let chain_id = network_named(c["network"].as_str().unwrap()).chain_id();
        let sig_msg = tx.sig_msg(&chain_id).unwrap();
        assert_eq!(sig_msg, hash(&c["sig_msg"]), "{label}");

        let secret: [u8; 32] = bytes(&c["secret"]).try_into().unwrap();
        let sender: [u8; 32] = bytes(&c["sender"]).try_into().unwrap();
        let signature: [u8; 64] = bytes(&c["signature"]).try_into().unwrap();
        assert_eq!(tx.sender().map(|p| p.0), Some(sender), "{label}");
        assert_eq!(yuanlai_crypto::public_key(&secret), sender, "{label}");
        assert_eq!(
            yuanlai_crypto::sign(&secret, sig_msg.as_bytes()),
            signature,
            "{label}"
        );
        assert!(yuanlai_crypto::verify_signature(
            &sender,
            sig_msg.as_bytes(),
            &signature
        ));
    }
    for c in cases(&v, "transfer_loss") {
        let got = yuanlai_consensus::transfer_loss(yuanlai_types::Credit(num(&c["amount"])));
        assert_eq!(got, Some(yuanlai_types::Credit(num(&c["loss"]))));
    }
    for c in cases(&v, "field_elements") {
        let value: [u8; 32] = bytes(&c["value"]).try_into().unwrap();
        assert_eq!(
            yuanlai_consensus::is_canonical_field_element(&value),
            c["canonical"].as_bool().unwrap()
        );
    }
    for c in cases(&v, "poseidon_commitments") {
        let secret: [u8; 32] = bytes(&c["secret"]).try_into().unwrap();
        let salt: [u8; 32] = bytes(&c["salt"]).try_into().unwrap();
        let expected = hash(&c["commitment"]);
        assert_eq!(
            yuanlai_zk::identity_commitment(&secret, &salt).unwrap(),
            expected
        );
    }
}

#[test]
fn course_vectors() {
    use yuanlai_consensus::{
        PathError, cancel_fee, check_path, course_cost, course_root, lock_bounds,
    };
    use yuanlai_types::{AutoCancelRecord, Credit, FileEntry, PubKey};

    let v = load("course.json");
    for c in cases(&v, "course_roots") {
        let mut entries: Vec<FileEntry> = c["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| FileEntry {
                path: BoundedText::new(e["path"].as_str().unwrap().to_owned()).unwrap(),
                content_hash: hash(&e["content_hash"]),
                size: num(&e["size"]),
            })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(course_root(&entries), hash(&c["root"]));
    }
    for c in cases(&v, "pricing") {
        let cost = course_cost(
            c["update"].as_bool().unwrap(),
            num(&c["files"]),
            num(&c["bytes"]),
        )
        .unwrap();
        assert_eq!(cost, Credit(num(&c["cost"])));
        let bounds = c["lock_bounds"].as_array().unwrap();
        assert_eq!(
            lock_bounds(cost),
            Some((Credit(num(&bounds[0])), Credit(num(&bounds[1]))))
        );
    }
    for c in cases(&v, "cancel_fees") {
        assert_eq!(
            cancel_fee(Credit(num(&c["locked"]))),
            Some(Credit(num(&c["fee"])))
        );
    }
    for c in cases(&v, "paths") {
        let got = check_path(c["path"].as_str().unwrap())
            .err()
            .map(|e| match e {
                PathError::Shape => "CRS-PATH-01",
                PathError::DotSegment => "CRS-PATH-02",
                PathError::Backslash => "CRS-PATH-03",
            });
        assert_eq!(got, c["error"].as_str(), "{}", c["path"]);
    }
    let r = &v["auto_cancel_record"];
    let record = AutoCancelRecord {
        session_id: hash(&r["session_id"]),
        owner: PubKey(bytes(&r["owner"]).try_into().unwrap()),
        locked: Credit(num(&r["locked"])),
        fee: Credit(num(&r["fee"])),
        restored: Credit(num(&r["restored"])),
    };
    assert_eq!(record.encode(), bytes(&r["encoding"]));
}

#[test]
fn course_tx_vectors() {
    let v = load("course-tx.json");
    for c in cases(&v, "cases") {
        let label = c["type"].as_str().unwrap();
        let encoding = bytes(&c["encoding"]);
        let tx = decode_exact::<Tx>(&encoding).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(tx.encode(), encoding, "{label}");
        assert_eq!(tx.txid(), hash(&c["txid"]), "{label}");
        let sig_msg = tx.sig_msg(&Network::Regtest.chain_id()).unwrap();
        assert_eq!(sig_msg, hash(&c["sig_msg"]), "{label}");
        let secret: [u8; 32] = bytes(&c["secret"]).try_into().unwrap();
        assert_eq!(
            yuanlai_crypto::sign(&secret, sig_msg.as_bytes()).to_vec(),
            bytes(&c["signature"])
        );
    }
}
