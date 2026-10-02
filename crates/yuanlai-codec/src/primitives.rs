use crate::{Decode, DecodeError, Encode, Reader};

macro_rules! impl_le_int {
    ($($t:ty),*) => {$(
        impl Encode for $t {
            fn encode_to(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }

        impl Decode for $t {
            fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
                reader.take_array().map(<$t>::from_le_bytes)
            }
        }
    )*};
}

impl_le_int!(u8, u16, u32, u64);

impl Encode for bool {
    fn encode_to(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl Decode for bool {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::decode_from(reader)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(DecodeError::InvalidBool(other)),
        }
    }
}

impl<const N: usize> Encode for [u8; N] {
    fn encode_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
}

impl<const N: usize> Decode for [u8; N] {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        reader.take_array()
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode_to(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                value.encode_to(out);
            }
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::decode_from(reader)? {
            0 => Ok(None),
            1 => T::decode_from(reader).map(Some),
            other => Err(DecodeError::InvalidOptionTag(other)),
        }
    }
}
