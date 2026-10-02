//! Pending transaction pool for YuanLai nodes. Policy only; never consensus-critical.

use std::collections::BTreeSet;
use std::fmt;

use yuanlai_consensus::{BlockError, Chain};
use yuanlai_types::{Hash, PubKey, Tx};

/// A transaction could not be admitted to the pending pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// System transactions are constructed by consensus, not submitted by users.
    SystemTransaction,
    /// This transaction ID is already pending.
    Duplicate,
    /// A candidate block template could not be constructed.
    TemplateUnavailable,
    /// The complete candidate block body violates a consensus rule.
    InvalidBlock(BlockError),
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemTransaction => f.write_str("system transaction cannot be submitted"),
            Self::Duplicate => f.write_str("transaction is already pending"),
            Self::TemplateUnavailable => f.write_str("cannot construct a block template"),
            Self::InvalidBlock(error) => write!(f, "invalid candidate block: {error}"),
        }
    }
}

impl core::error::Error for SubmitError {}

/// Ordered user transactions awaiting inclusion in a block.
#[derive(Debug, Clone, Default)]
pub struct Mempool {
    transactions: Vec<Tx>,
    transaction_ids: BTreeSet<Hash>,
}

impl Mempool {
    /// Checks and appends a transaction against the current chain and all pending transactions.
    ///
    /// Transactions retain submission order, so account nonces and same-block dependencies are
    /// validated exactly as they will be applied in the block.
    ///
    /// # Errors
    ///
    /// Returns [`SubmitError`] for system transactions, duplicates, unavailable templates, or a
    /// candidate body rejected by consensus.
    pub fn submit(
        &mut self,
        chain: &Chain,
        beneficiary: PubKey,
        transaction: Tx,
    ) -> Result<(), SubmitError> {
        if transaction.is_system() {
            return Err(SubmitError::SystemTransaction);
        }
        let id = transaction.txid();
        if self.transaction_ids.contains(&id) {
            return Err(SubmitError::Duplicate);
        }

        let mut candidate = self.transactions.clone();
        candidate.push(transaction.clone());
        let block = chain
            .template_with(
                beneficiary,
                yuanlai_codec::BoundedBytes::empty(),
                chain.min_next_timestamp(),
                candidate,
            )
            .ok_or(SubmitError::TemplateUnavailable)?;
        chain
            .check_body(&block)
            .map_err(SubmitError::InvalidBlock)?;

        self.transactions.push(transaction);
        self.transaction_ids.insert(id);
        Ok(())
    }

    /// User transactions in submission order.
    #[must_use]
    pub fn transactions(&self) -> &[Tx] {
        &self.transactions
    }

    /// Number of pending transactions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Whether the pool has no pending transactions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// Removes transactions after their candidate block has been connected.
    pub fn clear(&mut self) {
        self.transactions.clear();
        self.transaction_ids.clear();
    }
}

#[cfg(test)]
mod tests {
    use yuanlai_codec::{BoundedBytes, BoundedVec};
    use yuanlai_consensus::{Chain, genesis::genesis_block};
    use yuanlai_crypto::{Hash, public_key, sign};
    use yuanlai_types::{
        Coinbase, Credit, Height, Network, PubKey, RegisterIdentity, Signature, Tx, UserTx,
    };

    use super::{Mempool, SubmitError};

    const BENEFICIARY: PubKey = PubKey([
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ]);

    fn chain() -> Result<Chain, Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let genesis = genesis_block(network).ok_or("regtest genesis is missing")?;
        Ok(Chain::new(network.params(), &genesis)?)
    }

    fn connect_mined_blocks(
        chain: &mut Chain,
        beneficiary: PubKey,
        count: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..count {
            let mut block = chain
                .template(
                    beneficiary,
                    BoundedBytes::empty(),
                    chain.min_next_timestamp(),
                )
                .ok_or("could not build regtest template")?;
            block.header = yuanlai_miner::solve(&block.header, 0..=10_000)
                .ok_or("could not solve regtest block")?;
            chain.connect(&block)?;
        }
        Ok(())
    }

    #[test]
    fn rejects_system_transactions() -> Result<(), Box<dyn std::error::Error>> {
        let mut pool = Mempool::default();
        let tx = Tx::Coinbase(Coinbase {
            height: Height(1),
            beneficiary: BENEFICIARY,
            extra: BoundedBytes::empty(),
        });
        assert_eq!(
            pool.submit(&chain()?, BENEFICIARY, tx),
            Err(SubmitError::SystemTransaction)
        );
        assert!(pool.is_empty());
        Ok(())
    }

    #[test]
    fn rejects_invalid_transaction_without_mutating_pool() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut pool = Mempool::default();
        let tx = Tx::RegisterIdentity(UserTx {
            sender: BENEFICIARY,
            nonce: 0,
            body: RegisterIdentity {
                inputs: BoundedVec::empty(),
                commitment: Hash::ZERO,
            },
            signature: Signature::EMPTY,
        });
        assert!(matches!(
            pool.submit(&chain()?, BENEFICIARY, tx),
            Err(SubmitError::InvalidBlock(_))
        ));
        assert!(pool.is_empty());
        Ok(())
    }

    #[test]
    fn admits_signed_transaction_and_connects_it_from_template()
    -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let mut chain = chain()?;
        connect_mined_blocks(&mut chain, BENEFICIARY, 12)?;

        let secret = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ];
        assert_eq!(PubKey(public_key(&secret)), BENEFICIARY);
        let height = Height(chain.height().0.saturating_add(1));
        let inputs = chain
            .state()
            .select_inputs(&BENEFICIARY, Credit(20), height, |_| true)
            .ok_or("mature credits are missing")?;
        let mut transaction = Tx::RegisterIdentity(UserTx {
            sender: BENEFICIARY,
            nonce: chain.state().nonce(&BENEFICIARY),
            body: RegisterIdentity {
                inputs: BoundedVec::new(inputs)?,
                commitment: Hash([1; 32]),
            },
            signature: Signature::EMPTY,
        });
        let message = transaction
            .sig_msg(&network.chain_id())
            .ok_or("user transaction has no signing message")?;
        transaction.set_signature(Signature(sign(&secret, message.as_bytes())));

        let mut pool = Mempool::default();
        pool.submit(&chain, BENEFICIARY, transaction.clone())?;
        assert_eq!(pool.len(), 1);
        assert_eq!(
            pool.submit(&chain, BENEFICIARY, transaction.clone()),
            Err(SubmitError::Duplicate)
        );

        let mut block = chain
            .template_with(
                BENEFICIARY,
                BoundedBytes::empty(),
                chain.min_next_timestamp(),
                pool.transactions().to_vec(),
            )
            .ok_or("could not build transaction block template")?;
        block.header = yuanlai_miner::solve(&block.header, 0..=10_000)
            .ok_or("could not solve transaction block")?;
        chain.connect(&block)?;
        assert_eq!(chain.state().identity(&BENEFICIARY), Some(Hash([1; 32])));
        pool.clear();
        assert!(pool.is_empty());
        Ok(())
    }
}
