use core::fmt;

use crate::{Decode, DecodeError, Encode, Reader};

/// A collection exceeded its declared bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthError {
    /// The declared bound.
    pub max: u32,
    /// The actual length.
    pub found: usize,
}

impl fmt::Display for LengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "length {} exceeds maximum {}", self.found, self.max)
    }
}

impl core::error::Error for LengthError {}

/// Text rejected by CODEC-TEXT-02 or by its length bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// The UTF-8 encoding is longer than the bound.
    TooLong(LengthError),
    /// CODEC-TEXT-02: the text contains a C0/C1 control character or U+007F.
    ControlCharacter,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong(e) => write!(f, "text too long: {e}"),
            Self::ControlCharacter => {
                f.write_str("CODEC-TEXT-02: text contains a control character")
            }
        }
    }
}

impl core::error::Error for TextError {}

fn checked_len(len: usize, max: u32) -> Result<u32, LengthError> {
    u32::try_from(len)
        .ok()
        .filter(|l| *l <= max)
        .ok_or(LengthError { max, found: len })
}

fn decode_len(reader: &mut Reader<'_>, max: u32) -> Result<u32, DecodeError> {
    let found = u32::decode_from(reader)?;
    if found > max {
        return Err(DecodeError::LengthExceeded { max, found });
    }
    Ok(found)
}

/// `List<T, MAX>`: a `u32` element count followed by the elements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedVec<T, const MAX: u32> {
    items: Vec<T>,
    // Cached at construction so that encoding needs no fallible conversion.
    len: u32,
}

impl<T, const MAX: u32> BoundedVec<T, MAX> {
    /// Wraps `items` if their count is within `MAX`.
    ///
    /// # Errors
    ///
    /// Returns [`LengthError`] if `items.len() > MAX`.
    pub fn new(items: Vec<T>) -> Result<Self, LengthError> {
        let len = checked_len(items.len(), MAX)?;
        Ok(Self { items, len })
    }

    /// An empty list.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            items: Vec::new(),
            len: 0,
        }
    }

    /// Number of elements.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// Whether the list has no elements.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The elements as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Iterates over the elements.
    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Unwraps into the underlying vector.
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.items
    }
}

impl<'a, T, const MAX: u32> IntoIterator for &'a BoundedVec<T, MAX> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<T: Encode, const MAX: u32> Encode for BoundedVec<T, MAX> {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.len.encode_to(out);
        for item in &self.items {
            item.encode_to(out);
        }
    }
}

impl<T: Decode, const MAX: u32> Decode for BoundedVec<T, MAX> {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let len = decode_len(reader, MAX)?;
        // Every element occupies at least one byte, so this never over-allocates on hostile input.
        let capacity = usize::try_from(len)
            .unwrap_or(usize::MAX)
            .min(reader.remaining());
        let mut items = Vec::with_capacity(capacity);
        for _ in 0..len {
            items.push(T::decode_from(reader)?);
        }
        Ok(Self { items, len })
    }
}

/// `VarBytes<MAX>`: a `u32` byte length followed by raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedBytes<const MAX: u32> {
    bytes: Vec<u8>,
    len: u32,
}

impl<const MAX: u32> BoundedBytes<MAX> {
    /// Wraps `bytes` if their length is within `MAX`.
    ///
    /// # Errors
    ///
    /// Returns [`LengthError`] if `bytes.len() > MAX`.
    pub fn new(bytes: Vec<u8>) -> Result<Self, LengthError> {
        let len = checked_len(bytes.len(), MAX)?;
        Ok(Self { bytes, len })
    }

    /// An empty byte string.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            len: 0,
        }
    }

    /// Length in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// Whether the byte string is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl<const MAX: u32> Encode for BoundedBytes<MAX> {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.len.encode_to(out);
        out.extend_from_slice(&self.bytes);
    }
}

impl<const MAX: u32> Decode for BoundedBytes<MAX> {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let len = decode_len(reader, MAX)?;
        let n = usize::try_from(len).map_err(|_| DecodeError::UnexpectedEnd)?;
        let bytes = reader.take(n)?.to_vec();
        Ok(Self { bytes, len })
    }
}

/// `Text<MAX>`: UTF-8 without control characters, encoded like `VarBytes<MAX>`.
///
/// Compared byte-wise; no Unicode normalization is applied (see `spec/rationale.md`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedText<const MAX: u32> {
    text: String,
    len: u32,
}

fn has_control_character(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}'))
}

impl<const MAX: u32> BoundedText<MAX> {
    /// Wraps `text` if it satisfies CODEC-TEXT-02 and its UTF-8 length is within `MAX`.
    ///
    /// # Errors
    ///
    /// Returns [`TextError`] describing the violated rule.
    pub fn new(text: String) -> Result<Self, TextError> {
        let len = checked_len(text.len(), MAX).map_err(TextError::TooLong)?;
        if has_control_character(&text) {
            return Err(TextError::ControlCharacter);
        }
        Ok(Self { text, len })
    }

    /// Length of the UTF-8 encoding in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// The empty text; also the smallest value in byte order.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            text: String::new(),
            len: 0,
        }
    }

    /// Whether the text is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The text as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl<const MAX: u32> Encode for BoundedText<MAX> {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.len.encode_to(out);
        out.extend_from_slice(self.text.as_bytes());
    }
}

impl<const MAX: u32> Decode for BoundedText<MAX> {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let len = decode_len(reader, MAX)?;
        let n = usize::try_from(len).map_err(|_| DecodeError::UnexpectedEnd)?;
        let raw = reader.take(n)?;
        let text = core::str::from_utf8(raw).map_err(|_| DecodeError::InvalidText)?;
        if has_control_character(text) {
            return Err(DecodeError::InvalidText);
        }
        Ok(Self {
            text: text.to_owned(),
            len,
        })
    }
}
