use core::fmt;

use yuanlai_types::{Height, Target, Timestamp};

/// The caller supplied an incomplete branch. Indicates a bug in the caller, not an invalid block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    /// No timestamp is available for this height on the branch.
    MissingAncestor(Height),
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAncestor(h) => write!(f, "branch has no block at height {h}"),
        }
    }
}

impl core::error::Error for ContextError {}

/// A header violates a rule of `spec/02-block.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    /// BLK-02
    UnsupportedVersion(u32),
    /// BLK-03
    WrongChain,
    /// BLK-04: height is not parent height + 1.
    HeightMismatch {
        /// Parent height + 1.
        expected: Height,
        /// Height in the header.
        found: Height,
    },
    /// BLK-04: `prev_block` does not match the supplied parent.
    ParentMismatch,
    /// BLK-05
    WrongTarget {
        /// Target required by `NextTarget`.
        expected: Target,
        /// Target in the header.
        found: Target,
    },
    /// BLK-06
    InsufficientWork,
    /// BLK-07
    TimestampNotAfterMedian {
        /// Median time past of the parent.
        median: Timestamp,
        /// Timestamp in the header.
        found: Timestamp,
    },
    /// BLK-08
    TimeWarp,
    /// Genesis header is not at height 0 with an all-zero parent (`spec/02-block.md` §6).
    InvalidGenesis,
    /// The branch context was incomplete.
    Context(ContextError),
}

impl HeaderError {
    /// The spec rule identifier this error corresponds to.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion(_) => "BLK-02",
            Self::WrongChain => "BLK-03",
            Self::HeightMismatch { .. } | Self::ParentMismatch => "BLK-04",
            Self::WrongTarget { .. } => "BLK-05",
            Self::InsufficientWork => "BLK-06",
            Self::TimestampNotAfterMedian { .. } => "BLK-07",
            Self::TimeWarp => "BLK-08",
            Self::InvalidGenesis => "GENESIS",
            Self::Context(_) => "CONTEXT",
        }
    }
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.rule_id())?;
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported header version {v}"),
            Self::WrongChain => f.write_str("chain_id does not match this network"),
            Self::HeightMismatch { expected, found } => {
                write!(f, "height {found}, expected {expected}")
            }
            Self::ParentMismatch => f.write_str("prev_block does not match parent"),
            Self::WrongTarget { expected, found } => {
                write!(f, "target {}, expected {}", found.0, expected.0)
            }
            Self::InsufficientWork => f.write_str("block id exceeds target"),
            Self::TimestampNotAfterMedian { median, found } => {
                write!(f, "timestamp {found} not after median time past {median}")
            }
            Self::TimeWarp => f.write_str("retarget block timestamp too far before parent"),
            Self::InvalidGenesis => f.write_str("invalid genesis header"),
            Self::Context(e) => e.fmt(f),
        }
    }
}

impl core::error::Error for HeaderError {}

impl From<ContextError> for HeaderError {
    fn from(e: ContextError) -> Self {
        Self::Context(e)
    }
}
