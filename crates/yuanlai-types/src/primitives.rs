use core::fmt;

use yuanlai_codec::{Decode, DecodeError, Encode, Reader};

use crate::U256;

macro_rules! u64_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub u64);

        impl Encode for $name {
            fn encode_to(&self, out: &mut Vec<u8>) {
                self.0.encode_to(out);
            }
        }

        impl Decode for $name {
            fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
                u64::decode_from(reader).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

u64_newtype!(
    /// A block height. The genesis block has height 0.
    Height
);

u64_newtype!(
    /// Unix time in seconds.
    Timestamp
);

u64_newtype!(
    /// An amount of registration credit. Indivisible.
    Credit
);

impl Credit {
    /// Zero credit.
    pub const ZERO: Self = Self(0);

    /// `self + rhs`, or `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// `self − rhs`, or `None` if `rhs > self`.
    #[must_use]
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// `self − rhs`, or zero if `rhs > self`.
    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Height {
    /// `self + n`, or `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, n: u64) -> Option<Self> {
        match self.0.checked_add(n) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

/// An Ed25519 public key in compressed form. Validity is checked where it is used, not on decode.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PubKey(pub [u8; 32]);

impl fmt::Debug for PubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PubKey(")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

impl Encode for PubKey {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.0.encode_to(out);
    }
}

impl Decode for PubKey {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        reader.take_array().map(Self)
    }
}

/// A proof-of-work target, encoded as 32 big-endian bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Target(pub U256);

impl Encode for Target {
    fn encode_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_be_bytes());
    }
}

impl Decode for Target {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        reader
            .take_array()
            .map(|bytes| Self(U256::from_be_bytes(bytes)))
    }
}
