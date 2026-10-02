//! Consensus rules of the YuanLai protocol.
//!
//! Normative definitions: `spec/02-block.md` through `spec/05-course-session.md`. Implemented so
//! far: the header chain (P0) and the credit state machine with system transactions (P1).

mod ancestry;
mod block;
mod chain;
mod course;
mod course_tx;
mod delta;
mod difficulty;
mod error;
pub mod genesis;
mod header;
mod header_tree;
mod journal;
pub mod policy;
mod pow;
mod state;
mod time;
mod user_tx;

pub use ancestry::Ancestry;
pub use block::{
    BlockError, assemble, auto_cancel_commitment, expire_commitment, system_transactions,
    validate_block, validate_body, validate_genesis,
};
pub use chain::{Chain, ReorgError};
pub use course::{
    CourseDelta, CourseState, PathError, Registration, Session, cancel_fee, check_path,
    course_cost, course_root, gib_units, lock_bounds,
};
pub use course_tx::CourseError;
pub use delta::{BlockDelta, TransferEntry};
pub use difficulty::next_target;
pub use error::{ContextError, HeaderError};
pub use header::{validate_genesis_header, validate_header};
pub use header_tree::{Branch, HeaderTree, InsertOutcome, TreeError};
pub use journal::Change;
pub use pow::{block_work, meets_target};
pub use state::{AuthorVerification, ChainState, Totals, Violation};
pub use time::median_time_past;
pub use user_tx::{TxError, is_canonical_field_element, transfer_loss};
