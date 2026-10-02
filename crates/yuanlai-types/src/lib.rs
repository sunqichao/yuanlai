//! Core data types of the YuanLai protocol.
//!
//! Normative definitions: `spec/02-block.md`, `spec/03-transactions.md`, `spec/04-credit.md`.
//! User transactions arrive in P2.

mod block;
mod course;
mod network;
pub mod params;
mod primitives;
mod tx;
mod u256;
mod utxo;

pub use block::{Block, BlockHeader};
pub use course::{
    AddLock, AppendFiles, AutoCancelRecord, BeginCourse, Course, CoursePath, CourseTitle,
    CourseVersion, FileEntry, SessionRef,
};
pub use network::{ChainParams, Network};
pub use primitives::{Credit, Height, PubKey, Target, Timestamp};
pub use tx::{
    Coinbase, RecordCommitment, RegisterIdentity, Signature, Transfer, Tx, UserTx, VerifyAuthor,
};
pub use u256::U256;
pub use utxo::{ExpireRecord, OutPoint, Source, Utxo};
pub use yuanlai_crypto::Hash;
