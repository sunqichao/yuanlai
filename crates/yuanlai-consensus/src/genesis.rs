//! Genesis blocks (`spec/02-block.md` §6). Only timestamp, extra data and nonce are chosen; every
//! other field follows from the spec. Values are cross-checked against `spec/test-vectors/genesis.json`.

use yuanlai_codec::BoundedBytes;
use yuanlai_types::params::HEADER_VERSION;
use yuanlai_types::{
    Block, BlockHeader, Coinbase, Hash, Height, Network, PubKey, Target, Timestamp, Tx,
};

use crate::assemble;

struct GenesisParams {
    timestamp: u64,
    extra: &'static [u8],
    nonce: u64,
}

const fn params(network: Network) -> Option<GenesisParams> {
    match network {
        Network::Devnet => Some(GenesisParams {
            timestamp: 1_790_553_600,
            extra: b"YuanLai devnet genesis 2026-09-28",
            nonce: 50_481,
        }),
        Network::Regtest => Some(GenesisParams {
            timestamp: 1_790_553_600,
            extra: b"YuanLai regtest genesis 2026-09-28",
            nonce: 7,
        }),
        // Fixed at launch, with a public reference in `extra`.
        Network::Mainnet | Network::Testnet => None,
    }
}

/// The genesis block of `network`, or `None` if it has not been fixed yet.
#[must_use]
pub fn genesis_block(network: Network) -> Option<Block> {
    let p = params(network)?;
    let coinbase = Tx::Coinbase(Coinbase {
        height: Height(0),
        beneficiary: PubKey([0; 32]),
        extra: BoundedBytes::new(p.extra.to_vec()).ok()?,
    });
    let header = BlockHeader {
        version: HEADER_VERSION,
        chain_id: network.chain_id(),
        height: Height(0),
        prev_block: Hash::ZERO,
        tx_root: Hash::ZERO,
        timestamp: Timestamp(p.timestamp),
        target: Target(network.pow_limit()),
        nonce: p.nonce,
    };
    assemble(header, vec![coinbase])
}
