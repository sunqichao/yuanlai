//! Domain-separated hashing and Merkle trees for YuanLai.
//!
//! Normative definition: `spec/01-serialization.md` §4–§5. Ed25519 signatures arrive in P1.

mod ed25519;
mod hash;
mod merkle;

pub use ed25519::{is_valid_public_key, public_key, sign, verify_signature};
pub use hash::{Hash, Tag, TaggedHasher, sha256, tagged_hash};
pub use merkle::merkle_root;
