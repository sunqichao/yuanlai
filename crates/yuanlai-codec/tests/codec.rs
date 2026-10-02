//! Rule-by-rule tests for `spec/01-serialization.md`.

#![allow(
    clippy::unwrap_used,
    reason = "test code: any failure should abort the test"
)]

use proptest::prelude::*;
use yuanlai_codec::{
    BoundedBytes, BoundedText, BoundedVec, Decode, DecodeError, Encode, TextError, decode_exact,
};

type Bytes4 = BoundedBytes<4>;
type Text8 = BoundedText<8>;
type List3 = BoundedVec<u16, 3>;

#[test]
fn integers_are_little_endian_fixed_width() {
    assert_eq!(0x0102_u16.encode(), [0x02, 0x01]);
    assert_eq!(0x0102_0304_u32.encode(), [0x04, 0x03, 0x02, 0x01]);
    assert_eq!(1_u64.encode(), [1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(decode_exact::<u32>(&[4, 3, 2, 1]), Ok(0x0102_0304));
}

#[test]
fn dec_01_rejects_trailing_bytes() {
    assert_eq!(decode_exact::<u8>(&[1, 2]), Err(DecodeError::TrailingBytes));
    assert_eq!(DecodeError::TrailingBytes.rule_id(), "CODEC-DEC-01");
}

#[test]
fn dec_02_rejects_length_over_bound_before_reading_payload() {
    let input = [5, 0, 0, 0];
    assert_eq!(
        decode_exact::<Bytes4>(&input),
        Err(DecodeError::LengthExceeded { max: 4, found: 5 })
    );
    let huge_count = u32::MAX.encode();
    assert_eq!(
        decode_exact::<List3>(&huge_count),
        Err(DecodeError::LengthExceeded {
            max: 3,
            found: u32::MAX
        })
    );
}

#[test]
fn dec_03_rejects_non_canonical_bool_and_option() {
    assert_eq!(decode_exact::<bool>(&[0]), Ok(false));
    assert_eq!(decode_exact::<bool>(&[1]), Ok(true));
    assert_eq!(decode_exact::<bool>(&[2]), Err(DecodeError::InvalidBool(2)));
    assert_eq!(decode_exact::<Option<u8>>(&[0]), Ok(None));
    assert_eq!(decode_exact::<Option<u8>>(&[1, 7]), Ok(Some(7)));
    assert_eq!(
        decode_exact::<Option<u8>>(&[2, 7]),
        Err(DecodeError::InvalidOptionTag(2))
    );
}

#[test]
fn dec_05_rejects_invalid_utf8_and_control_characters() {
    let invalid_utf8 = [2, 0, 0, 0, 0xC3, 0x28];
    assert_eq!(
        decode_exact::<Text8>(&invalid_utf8),
        Err(DecodeError::InvalidText)
    );

    for control in ["\u{0}", "\u{1F}", "\u{7F}", "\u{80}", "\u{9F}"] {
        let mut input = u32::try_from(control.len()).unwrap().encode();
        input.extend_from_slice(control.as_bytes());
        assert_eq!(
            decode_exact::<Text8>(&input),
            Err(DecodeError::InvalidText),
            "{control:?}"
        );
        assert_eq!(
            Text8::new(control.to_owned()),
            Err(TextError::ControlCharacter)
        );
    }
}

#[test]
fn text_bound_counts_utf8_bytes_and_accepts_non_ascii() {
    let text = Text8::new("猿来".to_owned()).unwrap();
    assert_eq!(text.len(), 6);
    assert_eq!(decode_exact::<Text8>(&text.encode()), Ok(text));
    assert!(matches!(
        Text8::new("猿来猿".to_owned()),
        Err(TextError::TooLong(_))
    ));
}

#[test]
fn text_is_not_normalized() {
    let composed = Text8::new("\u{00E9}".to_owned()).unwrap();
    let decomposed = Text8::new("e\u{0301}".to_owned()).unwrap();
    assert_ne!(composed, decomposed);
    assert_ne!(composed.encode(), decomposed.encode());
}

#[test]
fn dec_06_rejects_truncated_input() {
    assert_eq!(
        decode_exact::<u64>(&[0; 7]),
        Err(DecodeError::UnexpectedEnd)
    );
    assert_eq!(
        decode_exact::<Bytes4>(&[3, 0, 0, 0, 1, 2]),
        Err(DecodeError::UnexpectedEnd)
    );
    assert_eq!(
        decode_exact::<List3>(&[2, 0, 0, 0, 1, 0]),
        Err(DecodeError::UnexpectedEnd)
    );
    assert_eq!(
        decode_exact::<[u8; 32]>(&[0; 31]),
        Err(DecodeError::UnexpectedEnd)
    );
}

#[test]
fn constructors_enforce_bounds() {
    assert!(Bytes4::new(vec![0; 4]).is_ok());
    assert!(Bytes4::new(vec![0; 5]).is_err());
    assert!(List3::new(vec![1, 2, 3]).is_ok());
    assert!(List3::new(vec![1, 2, 3, 4]).is_err());
}

#[test]
fn list_encoding_layout() {
    let list = List3::new(vec![1, 0x0203]).unwrap();
    assert_eq!(list.encode(), [2, 0, 0, 0, 1, 0, 3, 2]);
}

proptest! {
    #[test]
    fn roundtrip_u64(v: u64) {
        prop_assert_eq!(decode_exact::<u64>(&v.encode()), Ok(v));
    }

    #[test]
    fn roundtrip_bytes(v in proptest::collection::vec(any::<u8>(), 0..=4)) {
        let b = Bytes4::new(v).unwrap();
        prop_assert_eq!(decode_exact::<Bytes4>(&b.encode()), Ok(b));
    }

    #[test]
    fn roundtrip_option_list(v in proptest::option::of(proptest::collection::vec(any::<u16>(), 0..=3))) {
        let value = v.map(|items| List3::new(items).unwrap());
        prop_assert_eq!(decode_exact::<Option<List3>>(&value.encode()), Ok(value));
    }

    /// Any input that decodes must re-encode to exactly the same bytes (unique encoding).
    #[test]
    fn decode_then_encode_is_identity(input in proptest::collection::vec(any::<u8>(), 0..64)) {
        if let Ok(v) = decode_exact::<Pair<Option<List3>, Text8>>(&input) {
            prop_assert_eq!(v.encode(), input);
        }
    }
}

#[derive(Debug, PartialEq)]
struct Pair<A, B>(A, B);

impl<A: Encode, B: Encode> Encode for Pair<A, B> {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.0.encode_to(out);
        self.1.encode_to(out);
    }
}

impl<A: Decode, B: Decode> Decode for Pair<A, B> {
    fn decode_from(reader: &mut yuanlai_codec::Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(A::decode_from(reader)?, B::decode_from(reader)?))
    }
}
