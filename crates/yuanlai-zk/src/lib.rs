//! Zero-knowledge author identity for YuanLai, built on Halo2 without a trusted setup.
//!
//! Normative definition: `spec/06-zk-identity.md`.

use std::convert::TryFrom;
use std::sync::OnceLock;

use ff::PrimeField;
use halo2_gadgets::poseidon::{
    Hash as PoseidonGadget, Pow5Chip, Pow5Config,
    primitives::{self as poseidon, ConstantLength, P128Pow5T3},
};
use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    pasta::{EqAffine, Fp},
    plonk::{
        self, Advice, Circuit, Column, ConstraintSystem, Error as Halo2Error, Instance, ProvingKey,
        VerifyingKey,
    },
    poly::commitment::Params,
    transcript::{Blake2bRead, Blake2bWrite, Challenge255},
};
use rand_core::{CryptoRng, Rng};
use yuanlai_crypto::Hash as ProtocolHash;
use yuanlai_types::params::{IDENTITY_CIRCUIT_K, MAX_PROOF_SIZE};

const WIDTH: usize = 3;
const RATE: usize = 2;
const PUBLIC_INPUT_COUNT: usize = 6;

/// Errors returned by the identity commitment and proof APIs.
#[derive(Debug)]
pub enum IdentityError {
    /// A field element was not encoded canonically in little-endian form.
    NonCanonicalFieldElement,
    /// The requested Halo2 domain size is unsupported.
    UnsupportedDomainSize,
    /// The proof exceeds the protocol's maximum encoded size.
    ProofTooLarge,
    /// The fixed consensus verification key could not be initialized.
    SetupFailed,
    /// The proof transcript contains bytes after the canonical Halo2 proof.
    TrailingProofBytes,
    /// Halo2 rejected proof creation or verification.
    Halo2(Halo2Error),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonCanonicalFieldElement => {
                formatter.write_str("non-canonical Pallas field element")
            }
            Self::UnsupportedDomainSize => formatter.write_str("unsupported Halo2 domain size"),
            Self::ProofTooLarge => formatter.write_str("proof exceeds MAX_PROOF_SIZE"),
            Self::SetupFailed => {
                formatter.write_str("could not initialize author verification key")
            }
            Self::TrailingProofBytes => formatter.write_str("trailing bytes after Halo2 proof"),
            Self::Halo2(error) => write!(formatter, "Halo2 error: {error:?}"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<Halo2Error> for IdentityError {
    fn from(error: Halo2Error) -> Self {
        Self::Halo2(error)
    }
}

/// Transparent Halo2 parameters and keys for the fixed author-identity circuit.
#[derive(Debug)]
pub struct IdentityParameters {
    params: Params<EqAffine>,
    proving_key: ProvingKey<EqAffine>,
}

#[derive(Debug)]
struct VerificationParameters {
    params: Params<EqAffine>,
    verifying_key: VerifyingKey<EqAffine>,
}

/// A generated proof and its challenge/course-bound public nullifier.
#[derive(Debug)]
pub struct AuthorProof {
    /// Canonical little-endian Pallas field encoding of the nullifier.
    pub nullifier: ProtocolHash,
    /// Raw Halo2 proof transcript bytes.
    pub proof: Vec<u8>,
}

/// Compute the identity commitment `Poseidon(secret, salt)`.
///
/// # Errors
///
/// Returns [`IdentityError::NonCanonicalFieldElement`] if either input is not a canonical `Fp`.
pub fn identity_commitment(
    secret: &[u8; 32],
    salt: &[u8; 32],
) -> Result<ProtocolHash, IdentityError> {
    let secret = decode_field(secret)?;
    let salt = decode_field(salt)?;
    let commitment = poseidon::Hash::<_, P128Pow5T3, ConstantLength<2>, WIDTH, RATE>::init()
        .hash([secret, salt]);
    Ok(encode_field(commitment))
}

/// Generate deterministic circuit keys from transparent Halo2 parameters.
///
/// `k` must be in `6..32`. The returned parameters are generated from the
/// circuit and do not require a trusted setup ceremony.
///
/// # Errors
///
/// Returns [`IdentityError::UnsupportedDomainSize`] for unsupported `k`, or [`IdentityError::Halo2`]
/// if Halo2 key generation fails.
pub fn setup(k: u32) -> Result<IdentityParameters, IdentityError> {
    let params = checked_params(k)?;
    let circuit = AuthorCircuit::unknown();
    let verifying_key = plonk::keygen_vk(&params, &circuit)?;
    let proving_key = plonk::keygen_pk(&params, verifying_key, &circuit)?;
    Ok(IdentityParameters {
        params,
        proving_key,
    })
}

fn checked_params(k: u32) -> Result<Params<EqAffine>, IdentityError> {
    if !(6..32).contains(&k) {
        return Err(IdentityError::UnsupportedDomainSize);
    }
    Ok(Params::new(k))
}

fn setup_verification(k: u32) -> Result<VerificationParameters, IdentityError> {
    let params = checked_params(k)?;
    let verifying_key = plonk::keygen_vk(&params, &AuthorCircuit::unknown())?;
    Ok(VerificationParameters {
        params,
        verifying_key,
    })
}

/// Prove knowledge of the secret and salt for an identity and course.
///
/// # Errors
///
/// Returns an error for non-canonical secrets, Halo2 proving failures, or a proof exceeding
/// [`MAX_PROOF_SIZE`].
pub fn prove_author<R: CryptoRng + Rng>(
    parameters: &IdentityParameters,
    secret: &[u8; 32],
    salt: &[u8; 32],
    challenge: &ProtocolHash,
    course_root: &ProtocolHash,
    rng: R,
) -> Result<AuthorProof, IdentityError> {
    let secret_field = decode_field(secret)?;
    let salt_field = decode_field(salt)?;
    let commitment = identity_commitment(secret, salt)?;
    let challenge_parts = split_hash(challenge)?;
    let course_parts = split_hash(course_root)?;
    let nullifier_field = author_nullifier(secret_field, challenge_parts, course_parts);
    let nullifier = encode_field(nullifier_field);
    let public_inputs = public_inputs(
        decode_field(&commitment.0)?,
        challenge_parts,
        course_parts,
        nullifier_field,
    );
    let circuit = AuthorCircuit {
        secret: Value::known(secret_field),
        salt: Value::known(salt_field),
        challenge_parts: challenge_parts.map(Value::known),
        course_parts: course_parts.map(Value::known),
    };
    let instance_columns: &[&[Fp]] = &[&public_inputs[..]];
    let instances: &[&[&[Fp]]] = &[instance_columns];
    let mut transcript = Blake2bWrite::<_, EqAffine, Challenge255<_>>::init(Vec::new());
    plonk::create_proof(
        &parameters.params,
        &parameters.proving_key,
        &[circuit],
        instances,
        rng,
        &mut transcript,
    )?;
    let proof = transcript.finalize();
    let proof_limit = usize::try_from(MAX_PROOF_SIZE).map_err(|_| IdentityError::ProofTooLarge)?;
    if proof.len() > proof_limit {
        return Err(IdentityError::ProofTooLarge);
    }
    Ok(AuthorProof { nullifier, proof })
}

/// Verify an author proof against its identity, challenge, and course root.
///
/// # Errors
///
/// Returns an error for non-canonical public fields, oversized proofs, or invalid proofs.
pub fn verify_author(
    parameters: &IdentityParameters,
    commitment: &ProtocolHash,
    challenge: &ProtocolHash,
    course_root: &ProtocolHash,
    nullifier: &ProtocolHash,
    proof: &[u8],
) -> Result<(), IdentityError> {
    verify_with_key(
        &parameters.params,
        parameters.proving_key.get_vk(),
        commitment,
        challenge,
        course_root,
        nullifier,
        proof,
    )
}

/// Verify an author proof with the protocol's fixed consensus circuit parameters.
///
/// # Errors
///
/// Returns an error for invalid public fields, oversized or invalid proofs, or failed parameter
/// initialization.
pub fn verify_author_consensus(
    commitment: &ProtocolHash,
    challenge: &ProtocolHash,
    course_root: &ProtocolHash,
    nullifier: &ProtocolHash,
    proof: &[u8],
) -> Result<(), IdentityError> {
    static PARAMETERS: OnceLock<Result<VerificationParameters, String>> = OnceLock::new();
    let parameters = PARAMETERS
        .get_or_init(|| setup_verification(IDENTITY_CIRCUIT_K).map_err(|error| error.to_string()));
    let parameters = parameters
        .as_ref()
        .map_err(|_| IdentityError::SetupFailed)?;
    verify_with_key(
        &parameters.params,
        &parameters.verifying_key,
        commitment,
        challenge,
        course_root,
        nullifier,
        proof,
    )
}

fn verify_with_key(
    params: &Params<EqAffine>,
    verifying_key: &VerifyingKey<EqAffine>,
    commitment: &ProtocolHash,
    challenge: &ProtocolHash,
    course_root: &ProtocolHash,
    nullifier: &ProtocolHash,
    proof: &[u8],
) -> Result<(), IdentityError> {
    let proof_limit = usize::try_from(MAX_PROOF_SIZE).map_err(|_| IdentityError::ProofTooLarge)?;
    if proof.len() > proof_limit {
        return Err(IdentityError::ProofTooLarge);
    }
    let public_inputs = public_inputs(
        decode_field(&commitment.0)?,
        split_hash(challenge)?,
        split_hash(course_root)?,
        decode_field(&nullifier.0)?,
    );
    let instance_columns: &[&[Fp]] = &[&public_inputs[..]];
    let instances: &[&[&[Fp]]] = &[instance_columns];
    let strategy = plonk::SingleVerifier::new(params);
    let mut reader = std::io::Cursor::new(proof);
    let mut transcript = Blake2bRead::<_, EqAffine, Challenge255<_>>::init(&mut reader);
    plonk::verify_proof(params, verifying_key, strategy, instances, &mut transcript)?;
    let proof_length = u64::try_from(proof.len()).map_err(|_| IdentityError::ProofTooLarge)?;
    if reader.position() != proof_length {
        return Err(IdentityError::TrailingProofBytes);
    }
    Ok(())
}

fn decode_field(bytes: &[u8; 32]) -> Result<Fp, IdentityError> {
    Option::<Fp>::from(Fp::from_repr(*bytes)).ok_or(IdentityError::NonCanonicalFieldElement)
}

fn encode_field(value: Fp) -> ProtocolHash {
    ProtocolHash(value.to_repr())
}

fn half_hash_to_field(bytes: &[u8]) -> Result<Fp, IdentityError> {
    let mut representation = [0_u8; 32];
    let target = representation
        .get_mut(..bytes.len())
        .ok_or(IdentityError::NonCanonicalFieldElement)?;
    target.copy_from_slice(bytes);
    decode_field(&representation)
}

fn split_hash(hash: &ProtocolHash) -> Result<[Fp; 2], IdentityError> {
    let low = hash
        .0
        .get(..16)
        .ok_or(IdentityError::NonCanonicalFieldElement)?;
    let high = hash
        .0
        .get(16..32)
        .ok_or(IdentityError::NonCanonicalFieldElement)?;
    Ok([half_hash_to_field(low)?, half_hash_to_field(high)?])
}

fn author_nullifier(secret: Fp, challenge: [Fp; 2], course: [Fp; 2]) -> Fp {
    poseidon::Hash::<_, P128Pow5T3, ConstantLength<5>, WIDTH, RATE>::init().hash([
        secret,
        challenge[0],
        challenge[1],
        course[0],
        course[1],
    ])
}

fn public_inputs(
    commitment: Fp,
    challenge: [Fp; 2],
    course: [Fp; 2],
    nullifier: Fp,
) -> [Fp; PUBLIC_INPUT_COUNT] {
    [
        commitment,
        challenge[0],
        challenge[1],
        course[0],
        course[1],
        nullifier,
    ]
}

#[derive(Clone, Debug)]
struct AuthorCircuit {
    secret: Value<Fp>,
    salt: Value<Fp>,
    challenge_parts: [Value<Fp>; 2],
    course_parts: [Value<Fp>; 2],
}

impl AuthorCircuit {
    fn unknown() -> Self {
        Self {
            secret: Value::unknown(),
            salt: Value::unknown(),
            challenge_parts: [Value::unknown(), Value::unknown()],
            course_parts: [Value::unknown(), Value::unknown()],
        }
    }
}

#[derive(Clone, Debug)]
struct AuthorConfig {
    poseidon: Pow5Config<Fp, WIDTH, RATE>,
    witness_column: Column<Advice>,
    instance_column: Column<Instance>,
}

impl Circuit<Fp> for AuthorCircuit {
    type Config = AuthorConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self::unknown()
    }

    fn configure(meta: &mut ConstraintSystem<Fp>) -> Self::Config {
        let state = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];
        let partial_sbox = meta.advice_column();
        let rc_a = [
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
        ];
        let rc_b = [
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
        ];
        let instance_column = meta.instance_column();
        meta.enable_equality(instance_column);
        meta.enable_constant(rc_b[0]);
        let poseidon = Pow5Chip::configure::<P128Pow5T3>(meta, state, partial_sbox, rc_a, rc_b);
        AuthorConfig {
            poseidon,
            witness_column: state[0],
            instance_column,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<Fp>,
    ) -> Result<(), Halo2Error> {
        let cells = layouter.assign_region(
            || "author witness and public hash parts",
            |mut region| {
                let secret =
                    region.assign_advice(|| "secret", config.witness_column, 0, || self.secret)?;
                let salt =
                    region.assign_advice(|| "salt", config.witness_column, 1, || self.salt)?;
                let challenge_low = region.assign_advice(
                    || "challenge low",
                    config.witness_column,
                    2,
                    || self.challenge_parts[0],
                )?;
                let challenge_high = region.assign_advice(
                    || "challenge high",
                    config.witness_column,
                    3,
                    || self.challenge_parts[1],
                )?;
                let course_low = region.assign_advice(
                    || "course root low",
                    config.witness_column,
                    4,
                    || self.course_parts[0],
                )?;
                let course_high = region.assign_advice(
                    || "course root high",
                    config.witness_column,
                    5,
                    || self.course_parts[1],
                )?;
                Ok((
                    secret,
                    salt,
                    challenge_low,
                    challenge_high,
                    course_low,
                    course_high,
                ))
            },
        )?;
        let (secret, salt, challenge_low, challenge_high, course_low, course_high) = cells;
        layouter.constrain_instance(challenge_low.cell(), config.instance_column, 1)?;
        layouter.constrain_instance(challenge_high.cell(), config.instance_column, 2)?;
        layouter.constrain_instance(course_low.cell(), config.instance_column, 3)?;
        layouter.constrain_instance(course_high.cell(), config.instance_column, 4)?;

        let chip = Pow5Chip::construct(config.poseidon.clone());
        let commitment = PoseidonGadget::<Fp, _, P128Pow5T3, ConstantLength<2>, WIDTH, RATE>::init(
            chip,
            layouter.namespace(|| "commitment hash init"),
        )?
        .hash(
            layouter.namespace(|| "commitment hash"),
            [secret.clone(), salt],
        )?;
        layouter.constrain_instance(commitment.cell(), config.instance_column, 0)?;

        let nullifier_chip = Pow5Chip::construct(config.poseidon);
        let nullifier = PoseidonGadget::<Fp, _, P128Pow5T3, ConstantLength<5>, WIDTH, RATE>::init(
            nullifier_chip,
            layouter.namespace(|| "nullifier hash init"),
        )?
        .hash(
            layouter.namespace(|| "nullifier hash"),
            [
                secret,
                challenge_low,
                challenge_high,
                course_low,
                course_high,
            ],
        )?;
        layouter.constrain_instance(nullifier.cell(), config.instance_column, 5)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use halo2_proofs::{circuit::Value, dev::MockProver, pasta::Fp};
    use rand::{rand_core::UnwrapErr, rngs::SysRng};

    use super::{
        AuthorCircuit, IdentityError, author_nullifier, decode_field, identity_commitment,
        prove_author, public_inputs, setup, split_hash, verify_author,
    };
    use yuanlai_crypto::Hash;

    fn small_field(value: u8) -> [u8; 32] {
        let mut bytes = [0; 32];
        if let Some(first) = bytes.first_mut() {
            *first = value;
        }
        bytes
    }

    #[test]
    fn mock_prover_rejects_a_wrong_nullifier() -> Result<(), Box<dyn std::error::Error>> {
        let secret_bytes = small_field(17);
        let salt_bytes = small_field(29);
        let challenge = Hash([3; 32]);
        let course_root = Hash([4; 32]);
        let secret = decode_field(&secret_bytes)?;
        let salt = decode_field(&salt_bytes)?;
        let commitment = decode_field(&identity_commitment(&secret_bytes, &salt_bytes)?.0)?;
        let challenge_parts = split_hash(&challenge)?;
        let course_parts = split_hash(&course_root)?;
        let nullifier = author_nullifier(secret, challenge_parts, course_parts);
        let public = public_inputs(commitment, challenge_parts, course_parts, nullifier);
        let circuit = AuthorCircuit {
            secret: Value::known(secret),
            salt: Value::known(salt),
            challenge_parts: challenge_parts.map(Value::known),
            course_parts: course_parts.map(Value::known),
        };
        let prover = MockProver::run(9, &circuit, vec![public.to_vec()])?;
        prover
            .verify()
            .map_err(|_| "valid author circuit did not verify")?;

        let mut wrong_public = public;
        if let Some(last) = wrong_public.last_mut() {
            *last += Fp::from(1);
        }
        let invalid = MockProver::run(9, &circuit, vec![wrong_public.to_vec()])?;
        assert!(invalid.verify().is_err());
        Ok(())
    }

    #[test]
    fn proof_roundtrip_binds_identity_challenge_and_course()
    -> Result<(), Box<dyn std::error::Error>> {
        let parameters = setup(9)?;
        let secret = small_field(17);
        let salt = small_field(29);
        let challenge = Hash([3; 32]);
        let course_root = Hash([4; 32]);
        let commitment = identity_commitment(&secret, &salt)?;
        let author_proof = prove_author(
            &parameters,
            &secret,
            &salt,
            &challenge,
            &course_root,
            UnwrapErr(SysRng),
        )?;
        verify_author(
            &parameters,
            &commitment,
            &challenge,
            &course_root,
            &author_proof.nullifier,
            &author_proof.proof,
        )?;

        let mut proof_with_trailing_byte = author_proof.proof.clone();
        proof_with_trailing_byte.push(0);
        assert!(matches!(
            verify_author(
                &parameters,
                &commitment,
                &challenge,
                &course_root,
                &author_proof.nullifier,
                &proof_with_trailing_byte,
            ),
            Err(IdentityError::TrailingProofBytes)
        ));

        let wrong_challenge = Hash([5; 32]);
        assert!(
            verify_author(
                &parameters,
                &commitment,
                &wrong_challenge,
                &course_root,
                &author_proof.nullifier,
                &author_proof.proof,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn field_inputs_must_be_canonical() {
        assert!(matches!(
            identity_commitment(&[0xff; 32], &[1; 32]),
            Err(IdentityError::NonCanonicalFieldElement)
        ));
    }
}
