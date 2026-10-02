use yuanlai_crypto::{Hash, Tag, tagged_hash};

use crate::U256;
use crate::params::{POW_LIMIT, POW_LIMIT_REGTEST};

/// A YuanLai network (`spec/00-overview.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// `yuanlai-mainnet`
    Mainnet,
    /// `yuanlai-testnet`
    Testnet,
    /// `yuanlai-devnet`
    Devnet,
    /// `yuanlai-regtest`: local automated testing only; trivial proof of work.
    Regtest,
}

impl Network {
    /// The network name hashed into `chain_id`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mainnet => "yuanlai-mainnet",
            Self::Testnet => "yuanlai-testnet",
            Self::Devnet => "yuanlai-devnet",
            Self::Regtest => "yuanlai-regtest",
        }
    }

    /// Human-readable prefix for bech32m addresses (display only).
    #[must_use]
    pub const fn address_hrp(self) -> &'static str {
        match self {
            Self::Mainnet => "yl",
            Self::Testnet => "tyl",
            Self::Devnet => "dyl",
            Self::Regtest => "ryl",
        }
    }

    /// `TH("YuanLai/chain-id/v1", name)`.
    #[must_use]
    pub fn chain_id(self) -> Hash {
        tagged_hash(Tag::ChainId, self.name().as_bytes())
    }

    /// Easiest allowed target on this network.
    #[must_use]
    pub const fn pow_limit(self) -> U256 {
        match self {
            Self::Mainnet | Self::Testnet | Self::Devnet => POW_LIMIT,
            Self::Regtest => POW_LIMIT_REGTEST,
        }
    }

    /// Per-network consensus constants, with `chain_id` computed once.
    #[must_use]
    pub fn params(self) -> ChainParams {
        ChainParams {
            network: self,
            chain_id: self.chain_id(),
            pow_limit: self.pow_limit(),
        }
    }
}

/// Constants that differ between networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainParams {
    /// The network.
    pub network: Network,
    /// `network.chain_id()`.
    pub chain_id: Hash,
    /// `network.pow_limit()`.
    pub pow_limit: U256,
}
