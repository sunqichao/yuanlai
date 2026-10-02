use crate::DecodeError;

/// A cursor over input bytes that never reads past the end.
#[derive(Debug)]
pub struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Creates a reader over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    /// Number of unread bytes.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.remaining.len()
    }

    /// Whether all input has been consumed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    /// Consumes exactly `n` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::UnexpectedEnd`] if fewer than `n` bytes remain.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let (head, tail) = self
            .remaining
            .split_at_checked(n)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.remaining = tail;
        Ok(head)
    }

    /// Consumes exactly `N` bytes into an array.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::UnexpectedEnd`] if fewer than `N` bytes remain.
    pub fn take_array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (head, tail) = self
            .remaining
            .split_first_chunk::<N>()
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.remaining = tail;
        Ok(*head)
    }
}
