# ADR 0005: P4 Halo2 Identity Proofs

- Status: accepted
- Date: 2026-10-01

## Context

P2 reserves identity commitments as canonical Pallas base-field elements, and the transaction
specification reserves `VerifyAuthor` at tag `0x30`. P4 must define one deterministic circuit that
can prove knowledge of the registered identity secret without exposing it, while binding an
authorization challenge and a course root.

## Decisions

1. **Commitment hash:** use Halo2's `P128Pow5T3` Poseidon specification with `ConstantLength<2>`:
   `Poseidon(secret, salt)`. Both values are independent private Pallas base-field elements.
2. **Nullifier:** use the same Poseidon specification with `ConstantLength<5>` over the secret,
   two canonical 128-bit limbs of the challenge, and two limbs of the course root. This prevents
   replay across challenges and courses without exposing the secret.
3. **Proof system:** use Halo2 PLONK over Pallas with transparent deterministic parameters at
   `IDENTITY_CIRCUIT_K = 9`, the `Blake2bWrite`/`Blake2bRead` transcript and `Challenge255`. No
   trusted setup is required. The verifier key is initialized once and reused by consensus.
4. **Consensus transaction:** `VerifyAuthor` burns `COST_VERIFY_AUTHOR`, checks course ownership,
   identity existence, proof validity and one-time nullifier use. Verification records are indexed
   by nullifier and included in the block undo delta.
5. **Proof size:** the encoded transcript is bounded by `MAX_PROOF_SIZE`. The transaction field
   remains `VarBytes`, so its one `u32` length prefix is supplied by the normal transaction codec.

## Dependencies

- `halo2_proofs` 0.4 and `halo2_gadgets` 0.6 (MIT OR Apache-2.0): PLONK, Pallas and the constrained
  Poseidon gadget.
- `ff` 0.14 (MIT OR Apache-2.0): canonical Pallas field encoding.
- `rand_core` 0.10 (MIT OR Apache-2.0): caller-provided proving randomness; consensus verification
  does not use randomness.
- `rand` 0.10 (MIT OR Apache-2.0): operating-system randomness for proof generation and P2P
  connection nonces; tests also use its system RNG.

## Consequences

- `spec/06-zk-identity.md` is normative for the circuit, field encoding, public-input order and
  transcript.
- Proof-generation randomness may change proof bytes but cannot change whether a proof is valid or
  the resulting consensus state.
- The circuit and proof implementation require independent cryptographic review before production
  use. MockProver and end-to-end tests do not replace an audit.