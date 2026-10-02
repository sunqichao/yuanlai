//! Protocol parameters. Must match `spec/parameters.md` exactly; changing any value is a consensus change.

use crate::{Credit, U256};

/// Current block header version (BLK-02).
pub const HEADER_VERSION: u32 = 1;

/// Target seconds between blocks.
pub const TARGET_BLOCK_SPACING: u64 = 60;
/// Blocks per difficulty adjustment period.
pub const RETARGET_INTERVAL: u64 = 60;
/// Maximum factor by which one adjustment may change the target.
pub const MAX_RETARGET_FACTOR: u64 = 4;
/// Easiest allowed target: 2²³⁶ − 1 (block IDs need at least 20 leading zero bits).
pub const POW_LIMIT: U256 =
    U256::from_limbs_le([u64::MAX, u64::MAX, u64::MAX, 0x0000_0FFF_FFFF_FFFF]);
/// Easiest allowed target on regtest only: 2²⁵³ − 1.
pub const POW_LIMIT_REGTEST: U256 =
    U256::from_limbs_le([u64::MAX, u64::MAX, u64::MAX, 0x1FFF_FFFF_FFFF_FFFF]);
/// Number of ancestors in the median-time-past window.
pub const MTP_WINDOW: usize = 11;
/// Seconds a retarget-boundary block may precede its parent (BLK-08).
pub const TIMEWARP_GRACE: u64 = 600;
/// Seconds a block may be ahead of local time before acceptance is deferred (non-consensus).
pub const MAX_FUTURE_DRIFT: u64 = 600;

/// Maximum encoded block size in bytes.
pub const MAX_BLOCK_SIZE: usize = 1_048_576;
/// Maximum number of transactions in a block.
pub const MAX_BLOCK_TXS: u32 = 8_192;
/// Maximum encoded transaction size in bytes.
pub const MAX_TX_SIZE: usize = 131_072;
/// Maximum encoded author-identity proof size in bytes (TX-VA-02).
pub const MAX_PROOF_SIZE: u32 = 8_192;
/// Maximum user transactions from one sender in one block.
pub const MAX_USER_TXS_PER_SENDER_PER_BLOCK: u32 = 16;
/// Maximum length of `Coinbase.extra`.
pub const MAX_COINBASE_EXTRA: u32 = 64;
/// Maximum inputs of one transaction.
pub const MAX_INPUTS: u32 = 64;

/// Credit minted per block (before the balance cap).
pub const BLOCK_REWARD: Credit = Credit(10);
/// Blocks before a Coinbase output becomes spendable.
pub const COINBASE_MATURITY: u64 = 10;
/// Blocks a credit stays valid after it is obtained (about 24 hours).
pub const EXPIRY_BLOCKS: u64 = 1_440;
/// Maximum balance of one address, including immature and locked credit.
pub const BALANCE_CAP: Credit = Credit(500);

/// Percentage of a transfer that is burned.
pub const TRANSFER_LOSS_PERCENT: u64 = 10;
/// Minimum burned per transfer.
pub const TRANSFER_MIN_LOSS: Credit = Credit(1);
/// Maximum number of times a credit can be transferred.
pub const MAX_HOPS: u8 = 3;
/// Maximum number of validity renewals from ending course sessions.
pub const MAX_RENEWALS: u8 = 2;
/// Blocks during which a transfer cannot be sent back to its sender.
pub const NO_RETURN_WINDOW: u64 = 1_440;
/// Window for the inbound transfer cap.
pub const INBOUND_WINDOW: u64 = 1_440;
/// Maximum credit received by transfer within `INBOUND_WINDOW`.
pub const INBOUND_CAP: Credit = Credit(200);

/// Cost of `RegisterIdentity`.
pub const COST_IDENTITY: Credit = Credit(20);
/// Cost of `VerifyAuthor`.
pub const COST_VERIFY_AUTHOR: Credit = Credit(5);
/// Halo2 domain exponent for the fixed author-identity circuit.
pub const IDENTITY_CIRCUIT_K: u32 = 9;

/// Base cost of a new course.
pub const COST_COURSE_BASE: Credit = Credit(30);
/// Base cost of a new version of an existing course.
pub const COST_UPDATE_BASE: Credit = Credit(15);
/// Cost per (changed) file.
pub const COST_PER_FILE: Credit = Credit(1);
/// Cost per started GiB of (changed) content.
pub const COST_PER_GIB: Credit = Credit(5);
/// 2³⁰ bytes.
pub const GIB: u64 = 1 << 30;

/// Extra credit a session may lock above its estimate, in percent.
pub const LOCK_HEADROOM_PERCENT: u64 = 20;
/// Maximum lifetime of a course write session in blocks (about 7 days).
pub const SESSION_MAX_BLOCKS: u64 = 10_080;
/// Blocks without activity after which a session is cancelled automatically (about 24 hours).
pub const SESSION_IDLE_BLOCKS: u64 = 1_440;
/// Cancellation fee in percent of the locked credit.
pub const CANCEL_FEE_PERCENT: u64 = 5;
/// Minimum cancellation fee.
pub const CANCEL_MIN_FEE: Credit = Credit(2);
/// Manual cancellations allowed per address within `CANCEL_LIMIT_WINDOW`.
pub const CANCEL_LIMIT: u32 = 1;
/// Window for `CANCEL_LIMIT`.
pub const CANCEL_LIMIT_WINDOW: u64 = 1_440;
/// Maximum files in one course.
pub const MAX_FILES_PER_COURSE: u32 = 10_000;
/// Maximum file entries in one `AppendFiles`.
pub const MAX_FILES_PER_APPEND: u32 = 256;
/// Maximum UTF-8 length of a file path.
pub const MAX_PATH_BYTES: u32 = 512;
/// Maximum UTF-8 length of a course title.
pub const MAX_TITLE_BYTES: u32 = 256;
/// Maximum UTF-8 length of a course version label.
pub const MAX_VERSION_BYTES: u32 = 32;

/// Modulus of the Pallas base field, the native field of the halo2 identity circuit.
pub const CIRCUIT_FIELD_MODULUS: U256 = U256::from_limbs_le([
    0x992d_30ed_0000_0001,
    0x2246_98fc_094c_f91b,
    0x0000_0000_0000_0000,
    0x4000_0000_0000_0000,
]);
