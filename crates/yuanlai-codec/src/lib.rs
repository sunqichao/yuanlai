//! Canonical byte encoding for YuanLai consensus data.
//!
//! Normative definition: `spec/01-serialization.md`. Every value has exactly one valid
//! encoding, and [`decode_exact`] rejects everything else.

mod bounded;
mod error;
mod primitives;
mod reader;

pub use bounded::{BoundedBytes, BoundedText, BoundedVec, LengthError, TextError};
pub use error::DecodeError;
pub use reader::Reader;

/// A value with a canonical encoding.
pub trait Encode {
    /// Appends the canonical encoding of `self` to `out`.
    fn encode_to(&self, out: &mut Vec<u8>);

    /// Returns the canonical encoding of `self`.
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_to(&mut out);
        out
    }
}

/// A value that can be strictly decoded from its canonical encoding.
pub trait Decode: Sized {
    /// Decodes one value from the front of `reader`, leaving the rest unread.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] if the input is not a canonical encoding of `Self`.
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError>;
}

/// Decodes a value that must occupy all of `bytes`.
///
/// # Errors
///
/// Returns [`DecodeError::TrailingBytes`] (CODEC-DEC-01) if input remains after the value,
/// or any error produced while decoding the value itself.
pub fn decode_exact<T: Decode>(bytes: &[u8]) -> Result<T, DecodeError> {
    let mut reader = Reader::new(bytes);
    let value = T::decode_from(&mut reader)?;
    if reader.is_empty() {
        Ok(value)
    } else {
        Err(DecodeError::TrailingBytes)
    }
}
