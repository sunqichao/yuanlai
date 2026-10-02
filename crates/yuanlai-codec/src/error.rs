use core::fmt;

/// Reasons a byte string is not a canonical encoding. Each variant maps to a spec rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// CODEC-DEC-01: bytes remain after the value.
    TrailingBytes,
    /// CODEC-DEC-02: a length or element count exceeds the type's bound.
    LengthExceeded {
        /// The declared bound.
        max: u32,
        /// The length found in the input.
        found: u32,
    },
    /// CODEC-DEC-03: a `bool` byte other than `0x00` or `0x01`.
    InvalidBool(u8),
    /// CODEC-DEC-03: an `Option` tag other than `0x00` or `0x01`.
    InvalidOptionTag(u8),
    /// CODEC-DEC-04: an undefined enum tag.
    UnknownTag(u8),
    /// CODEC-DEC-05: text violates CODEC-TEXT-01 or CODEC-TEXT-02.
    InvalidText,
    /// CODEC-DEC-06: input ended in the middle of a field.
    UnexpectedEnd,
}

impl DecodeError {
    /// The spec rule identifier this error corresponds to.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::TrailingBytes => "CODEC-DEC-01",
            Self::LengthExceeded { .. } => "CODEC-DEC-02",
            Self::InvalidBool(_) | Self::InvalidOptionTag(_) => "CODEC-DEC-03",
            Self::UnknownTag(_) => "CODEC-DEC-04",
            Self::InvalidText => "CODEC-DEC-05",
            Self::UnexpectedEnd => "CODEC-DEC-06",
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.rule_id())?;
        match self {
            Self::TrailingBytes => f.write_str("trailing bytes after value"),
            Self::LengthExceeded { max, found } => {
                write!(f, "length {found} exceeds maximum {max}")
            }
            Self::InvalidBool(b) => write!(f, "invalid bool byte {b:#04x}"),
            Self::InvalidOptionTag(b) => write!(f, "invalid option tag {b:#04x}"),
            Self::UnknownTag(b) => write!(f, "unknown enum tag {b:#04x}"),
            Self::InvalidText => f.write_str("invalid text"),
            Self::UnexpectedEnd => f.write_str("unexpected end of input"),
        }
    }
}

impl core::error::Error for DecodeError {}
