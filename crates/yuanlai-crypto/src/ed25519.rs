use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Whether `bytes` is a valid public key: it decompresses to a curve point, re-compresses to the
/// same bytes (canonical encoding), and the point does not have small order (TX-CB-02).
#[must_use]
pub fn is_valid_public_key(bytes: &[u8; 32]) -> bool {
    VerifyingKey::from_bytes(bytes)
        .is_ok_and(|key| !key.is_weak() && key.to_edwards().compress().as_bytes() == bytes)
}

/// TX-SIG-01: `public_key` is valid and `signature` verifies under RFC 8032 with strict checks.
#[must_use]
pub fn verify_signature(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    is_valid_public_key(public_key)
        && VerifyingKey::from_bytes(public_key).is_ok_and(|key| {
            key.verify_strict(message, &Signature::from_bytes(signature))
                .is_ok()
        })
}

/// The public key of a 32-byte Ed25519 secret key.
#[must_use]
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(secret).verifying_key().to_bytes()
}

/// Deterministic RFC 8032 signature. Wallet-side only; consensus never signs.
#[must_use]
pub fn sign(secret: &[u8; 32], message: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(secret).sign(message).to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2²⁵⁵ − 19 in little-endian.
    const P: [u8; 32] = {
        let mut p = [0xFF; 32];
        p[0] = 0xED;
        p[31] = 0x7F;
        p
    };

    fn p_plus(k: u8) -> [u8; 32] {
        let mut out = P;
        out[0] = 0xED_u8.wrapping_add(k);
        out
    }

    fn small(y: u8) -> [u8; 32] {
        let mut out = [0; 32];
        out[0] = y;
        out
    }

    #[test]
    fn rfc8032_public_key_is_valid() {
        let key: [u8; 32] = [
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ];
        assert!(is_valid_public_key(&key));
    }

    #[test]
    fn small_order_points_are_rejected() {
        // y = 1 is the identity; y = 0 has order 4.
        assert!(!is_valid_public_key(&small(1)));
        assert!(!is_valid_public_key(&[0; 32]));
    }

    #[test]
    fn non_canonical_encodings_are_rejected() {
        // For k < 19, y = k + p encodes the same field element as y = k. Every such encoding that
        // decompresses to a large-order point must still be rejected, while its canonical form is
        // accepted.
        let mut checked = 0;
        for k in 0..19 {
            let non_canonical = p_plus(k);
            let decodes = VerifyingKey::from_bytes(&non_canonical).is_ok_and(|key| !key.is_weak());
            if decodes {
                assert!(!is_valid_public_key(&non_canonical), "k = {k}");
                assert!(is_valid_public_key(&small(k)), "k = {k}");
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no non-canonical large-order encoding exercised"
        );
    }

    #[test]
    fn off_curve_bytes_are_rejected() {
        let rejected = (0..=255_u8).filter(|y| VerifyingKey::from_bytes(&small(*y)).is_err());
        assert!(rejected.clone().count() > 0);
        for y in rejected {
            assert!(!is_valid_public_key(&small(y)));
        }
    }

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0; N];
        for (byte, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        out
    }

    /// RFC 8032 §7.1 tests 1 and 2.
    #[test]
    fn rfc8032_vectors() {
        let cases = [
            (
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                &[][..],
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
            ),
            (
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                &[0x72][..],
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
            ),
        ];
        for (secret, public, message, signature) in cases {
            let secret = unhex::<32>(secret);
            let public = unhex::<32>(public);
            let signature = unhex::<64>(signature);
            assert_eq!(public_key(&secret), public);
            assert_eq!(sign(&secret, message), signature);
            assert!(verify_signature(&public, message, &signature));
        }
    }

    #[test]
    fn strict_verification_rejects_tampering() {
        let secret = [7; 32];
        let public = public_key(&secret);
        let signature = sign(&secret, b"msg");
        assert!(verify_signature(&public, b"msg", &signature));
        assert!(!verify_signature(&public, b"msh", &signature));

        // S + L encodes the same scalar non-canonically; strict verification must reject it.
        let l: [u8; 32] = unhex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
        let mut malleated = signature;
        let mut carry = 0_u16;
        for (byte, add) in malleated[32..].iter_mut().zip(l) {
            let sum = u16::from(*byte) + u16::from(add) + carry;
            *byte = u8::try_from(sum & 0xFF).unwrap();
            carry = sum >> 8;
        }
        assert!(!verify_signature(&public, b"msg", &malleated));

        assert!(!verify_signature(&small(1), b"msg", &signature));
    }
}
