use core::cmp::Ordering;
use core::fmt;
use core::num::{NonZeroU64, NonZeroU128};

/// 256-bit unsigned integer for proof-of-work targets and chain work.
///
/// Limbs are stored least-significant first. Only the operations the protocol needs are provided,
/// and none of them can panic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct U256([u64; 4]);

#[expect(
    clippy::cast_possible_truncation,
    reason = "intentionally keeps the low 64 bits"
)]
const fn low64(v: u128) -> u64 {
    v as u64
}

const fn high64(v: u128) -> u64 {
    low64(v.wrapping_shr(64))
}

fn join(high: u64, low: u64) -> u128 {
    u128::from(high).wrapping_shl(64) | u128::from(low)
}

impl U256 {
    /// 0
    pub const ZERO: Self = Self([0; 4]);
    /// 1
    pub const ONE: Self = Self([1, 0, 0, 0]);
    /// 2²⁵⁶ − 1
    pub const MAX: Self = Self([u64::MAX; 4]);

    /// Builds a value from little-endian limbs.
    #[must_use]
    pub const fn from_limbs_le(limbs: [u64; 4]) -> Self {
        Self(limbs)
    }

    /// Builds a value from a `u64`.
    #[must_use]
    pub const fn from_u64(v: u64) -> Self {
        Self([v, 0, 0, 0])
    }

    /// Interprets 32 bytes as a big-endian integer (`spec/01-serialization.md` §6).
    #[must_use]
    pub fn from_be_bytes(bytes: [u8; 32]) -> Self {
        let (chunks, _) = bytes.as_chunks::<8>();
        let mut limbs = [0_u64; 4];
        for (limb, chunk) in limbs.iter_mut().zip(chunks.iter().rev()) {
            *limb = u64::from_be_bytes(*chunk);
        }
        Self(limbs)
    }

    /// Big-endian 32-byte representation.
    #[must_use]
    pub fn to_be_bytes(self) -> [u8; 32] {
        let mut out = [0_u8; 32];
        let (chunks, _) = out.as_chunks_mut::<8>();
        for (chunk, limb) in chunks.iter_mut().rev().zip(self.0) {
            *chunk = limb.to_be_bytes();
        }
        out
    }

    /// Whether the value is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self == Self::ZERO
    }

    /// `self + rhs`, or `None` on overflow.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let mut out = [0_u64; 4];
        let mut carry = false;
        for ((o, a), b) in out.iter_mut().zip(self.0).zip(rhs.0) {
            let (s1, c1) = a.overflowing_add(b);
            let (s2, c2) = s1.overflowing_add(u64::from(carry));
            *o = s2;
            carry = c1 || c2;
        }
        (!carry).then_some(Self(out))
    }

    /// `self − rhs`, or `None` if `rhs > self`.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        let (diff, borrow) = self.overflowing_sub(rhs);
        (!borrow).then_some(diff)
    }

    fn overflowing_sub(self, rhs: Self) -> (Self, bool) {
        let mut out = [0_u64; 4];
        let mut borrow = false;
        for ((o, a), b) in out.iter_mut().zip(self.0).zip(rhs.0) {
            let (d1, b1) = a.overflowing_sub(b);
            let (d2, b2) = d1.overflowing_sub(u64::from(borrow));
            *o = d2;
            borrow = b1 || b2;
        }
        (Self(out), borrow)
    }

    /// `self × rhs`, or `None` on overflow.
    #[must_use]
    pub fn checked_mul_u64(self, rhs: u64) -> Option<Self> {
        let mut out = [0_u64; 4];
        let mut carry = 0_u64;
        for (o, a) in out.iter_mut().zip(self.0) {
            // (2⁶⁴−1)² + (2⁶⁴−1) < 2¹²⁸, so this cannot wrap.
            let product = u128::from(a)
                .wrapping_mul(u128::from(rhs))
                .wrapping_add(u128::from(carry));
            *o = low64(product);
            carry = high64(product);
        }
        (carry == 0).then_some(Self(out))
    }

    /// Floor division and remainder by a non-zero `u64`.
    #[must_use]
    pub fn div_rem_u64(self, divisor: NonZeroU64) -> (Self, u64) {
        let d = NonZeroU128::from(divisor);
        let mut quotient = [0_u64; 4];
        let mut rem = 0_u64;
        for (q, n) in quotient.iter_mut().rev().zip(self.0.iter().rev()) {
            // rem < d ≤ 2⁶⁴−1, so cur < d·2⁶⁴ and the quotient digit fits in 64 bits.
            let cur = join(rem, *n);
            *q = low64(cur / d);
            rem = low64(cur % d);
        }
        (Self(quotient), rem)
    }

    /// Floor division and remainder, or `None` if `divisor` is zero.
    #[must_use]
    pub fn checked_div_rem(self, divisor: Self) -> Option<(Self, Self)> {
        if divisor.is_zero() {
            return None;
        }
        let mut quotient = [0_u64; 4];
        let mut rem = Self::ZERO;
        for (q, n) in quotient.iter_mut().rev().zip(self.0.iter().rev()) {
            for bit in (0..64_u32).rev() {
                let (shifted, overflow) = rem.shl1_with(n.wrapping_shr(bit) & 1);
                // With overflow the true remainder is 2²⁵⁶ + shifted > divisor, and the wrapped
                // subtraction still yields the correct value below divisor.
                if overflow || shifted >= divisor {
                    rem = shifted.overflowing_sub(divisor).0;
                    *q |= 1_u64.wrapping_shl(bit);
                } else {
                    rem = shifted;
                }
            }
        }
        Some((Self(quotient), rem))
    }

    /// `(self << 1) | low_bit`, and whether a bit was shifted out.
    fn shl1_with(self, low_bit: u64) -> (Self, bool) {
        let mut out = [0_u64; 4];
        let mut carry = low_bit;
        for (o, a) in out.iter_mut().zip(self.0) {
            *o = a.wrapping_shl(1) | carry;
            carry = a.wrapping_shr(63);
        }
        (Self(out), carry != 0)
    }
}

impl core::ops::Not for U256 {
    type Output = Self;

    fn not(self) -> Self {
        Self(self.0.map(|limb| !limb))
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "U256(0x{self})")
    }
}

impl fmt::Display for U256 {
    /// Fixed-width lowercase hexadecimal.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for limb in self.0.iter().rev() {
            write!(f, "{limb:016x}")?;
        }
        Ok(())
    }
}
