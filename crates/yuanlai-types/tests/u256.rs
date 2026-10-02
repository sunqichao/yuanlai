//! `U256` checked against `num-bigint` as an independent oracle.

#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    reason = "test code; oracle arithmetic is on arbitrary-precision BigUint, which cannot overflow"
)]

use core::num::NonZeroU64;

use num_bigint::BigUint;
use proptest::prelude::*;
use yuanlai_types::U256;

fn big(v: U256) -> BigUint {
    BigUint::from_bytes_be(&v.to_be_bytes())
}

fn two_pow_256() -> BigUint {
    BigUint::from(1_u8) << 256_u32
}

/// Biased toward edge values: zero, one, all-ones limbs and small numbers.
fn any_u256() -> impl Strategy<Value = U256> {
    let limb = prop_oneof![
        Just(0_u64),
        Just(1_u64),
        Just(u64::MAX),
        0_u64..16,
        any::<u64>(),
    ];
    [limb.clone(), limb.clone(), limb.clone(), limb].prop_map(U256::from_limbs_le)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn be_bytes_roundtrip(v in any_u256()) {
        prop_assert_eq!(U256::from_be_bytes(v.to_be_bytes()), v);
    }

    #[test]
    fn ordering_matches(a in any_u256(), b in any_u256()) {
        prop_assert_eq!(a.cmp(&b), big(a).cmp(&big(b)));
    }

    #[test]
    fn add_matches(a in any_u256(), b in any_u256()) {
        let expected = big(a) + big(b);
        match a.checked_add(b) {
            Some(sum) => prop_assert_eq!(big(sum), expected),
            None => prop_assert!(expected >= two_pow_256()),
        }
    }

    #[test]
    fn sub_matches(a in any_u256(), b in any_u256()) {
        match a.checked_sub(b) {
            Some(diff) => prop_assert_eq!(big(diff), big(a) - big(b)),
            None => prop_assert!(big(a) < big(b)),
        }
    }

    #[test]
    fn mul_u64_matches(a in any_u256(), b: u64) {
        let expected = big(a) * BigUint::from(b);
        match a.checked_mul_u64(b) {
            Some(product) => prop_assert_eq!(big(product), expected),
            None => prop_assert!(expected >= two_pow_256()),
        }
    }

    #[test]
    fn div_rem_u64_matches(a in any_u256(), d in 1_u64..) {
        let (q, r) = a.div_rem_u64(NonZeroU64::new(d).unwrap());
        prop_assert_eq!(big(q), big(a) / BigUint::from(d));
        prop_assert_eq!(BigUint::from(r), big(a) % BigUint::from(d));
    }

    #[test]
    fn div_rem_matches(a in any_u256(), d in any_u256()) {
        match a.checked_div_rem(d) {
            Some((q, r)) => {
                prop_assert_eq!(big(q), big(a) / big(d));
                prop_assert_eq!(big(r), big(a) % big(d));
            }
            None => prop_assert!(d.is_zero()),
        }
    }

    #[test]
    fn not_matches(a in any_u256()) {
        prop_assert_eq!(big(!a), (two_pow_256() - 1_u8) - big(a));
    }
}

#[test]
fn division_by_divisor_above_2_pow_255_exercises_overflow_path() {
    let divisor = U256::MAX.checked_sub(U256::from_u64(5)).unwrap();
    let (q, r) = U256::MAX.checked_div_rem(divisor).unwrap();
    assert_eq!(q, U256::ONE);
    assert_eq!(r, U256::from_u64(5));
}

#[test]
fn display_is_fixed_width_hex() {
    assert_eq!(
        U256::ONE.to_string(),
        "0000000000000000000000000000000000000000000000000000000000000001"
    );
}
