# ADR 0007: V2 Fixed-Validator BFT Engine Evaluation

- Status: proposed; single-validator interoperability PoC passed; four-validator P2P gate failed and remains unresolved
- Date: 2026-10-02

## Context

V2 requires a fixed validator set and read-only full nodes, with no mining, token,
fees, or economic voting power. Consensus must provide deterministic ordering,
commit certificates, validator-set handling, peer synchronization, and a clear
fault model. The workspace is a Rust application; operators should be able to run
the product without building their own consensus implementation.

This is an evaluation, not a protocol or production approval. Sources were checked
on 2026-10-02.

## Candidates

| Candidate | Assessment |
|---|---|
| CometBFT as an external consensus process with a Rust ABCI++ application | Preferred candidate for a compatibility PoC. The official docs describe generic BFT state-machine replication and a language-neutral ABCI interface. It can tolerate fewer than one-third Byzantine voting power, but makes deployment a multi-process/multi-runtime product and requires a deterministic application. |
| Implement BFT directly in YuanLai Rust | Rejected for the MVP. This would make consensus, safety proofs, timeout/locking behavior, membership updates, and adversarial testing our responsibility without a clear benefit over a mature engine. |
| Raft-based service such as etcd | Not suitable for the chosen threat model: crash-fault tolerance does not protect against Byzantine validators. |

CometBFT's documented commit rule requires more than two-thirds of voting power
to pre-commit the same block in the same round; its stated safety assumption is
less than one-third Byzantine. Its application logic must be deterministic.
ABCI++ uses Protobuf-defined request/response methods and supports applications
in other languages through gRPC or the socket protocol. Socket servers must
handle multiple connections. The official docs currently describe four ABCI
connections, so a custom Rust server would be security-sensitive protocol code.

The CometBFT release page showed v0.40.0 as latest, alongside maintenance
releases v0.39.4 and v0.38.26. The official Docker Hub v0.40.0 image provides
linux/arm64; its manifest digest is
`sha256:0703fb881dac0987c8e41a5810cc09dbcb503bb59ac91705f6936c4b972ac798`.
This establishes an available pinned runtime, not a security audit or a YuanLai
compatibility guarantee. The Rust `tendermint-rs` repository
provides data structures and ABCI-related crates, but its README's stated tested
compatibility is Tendermint Core v0.34.21; that is insufficient evidence for
ABCI++ compatibility with CometBFT v0.40. The `tower-abci` project is another
Rust ABCI server candidate, but its current compatibility with the required
CometBFT release and ABCI++ methods has not been established.

## Proposed Direction

Use CometBFT as the provisional engine candidate and keep YuanLai's registration
state machine in Rust behind ABCI++. Do not implement a custom BFT engine or
copy consensus logic from an adapter crate. Configure an explicit fixed validator
set with equal voting power unless governance later decides otherwise; voting
power is configuration, not a token or user balance. Read-only full nodes do not
hold validator signing keys.

No CometBFT version, ABCI transport, Rust adapter, validator count, key-rotation
scheme, membership governance, or time bound is frozen by this ADR. The v0.40.0
source declares Apache-2.0; verify the full dependency/license inventory,
supported platform strategy, release support policy, and security response
process before adopting a version.

The current development host has Rust and `protoc`, but no Go toolchain or
native CometBFT executable. The official arm64 image was run in Docker. Its
`version --verbose` output reports `0.39.0+0880b4d`, ABCI `2.0.0`, block protocol
11, and P2P protocol 8. The version constant at the v0.40.0 source tag declares
`TMCoreSemVer = "0.39.0"`, and the commit hash matches the release tag; pin the
image digest/tag and protocol versions rather than assuming the CLI's displayed
semver equals the Git release tag.

A temporary single-node smoke test using the image's built-in `kvstore` app
reached height 16 and reported `catching_up=false`; its container and volume were
removed afterward. This proves the pinned engine image starts on this host, not
that a Rust ABCI++ app or multi-validator network works. No Rust ABCI++ PoC has
run yet.

### Rust ABCI++ Interoperability Result

On 2026-10-02, the upstream `tower-abci` v0.19.1 `kvstore_38` example compiled
with `tendermint`/`tendermint-proto` 0.40.4 and connected to the pinned CometBFT
v0.40.0 arm64 image using the ABCI Socket transport. The engine continuously
committed blocks (`catching_up=false`); a JSON-encoded RPC transaction returned
`check_tx.code=0` and `tx_result.code=0`, and a subsequent ABCI query returned
the written key/value. The temporary node, volume, and server were stopped and
removed after the test.

This establishes single-validator wire/API interoperability for the published
adapter and this engine image. It does not establish multi-validator safety,
deterministic application behavior under replay, persistent state/restart,
snapshot correctness, or production suitability. The example is an in-memory
demonstration and must not be reused as YuanLai ledger logic. The exact output
also confirms the engine reports CometBFT `0.39.0+0880b4d` while the ABCI
protocol reports `2.0.0`.

### Four-Validator P2P Attempt

A four-validator local network was attempted on 2026-10-02 using the same pinned
CometBFT image and four separate `tower-abci` in-memory demo applications. The
first run was invalid: the image entrypoint continued using its default
`/cometbft` home because the node home was supplied in the wrong CLI position;
the RPCs therefore showed the default `dockerchain` and one validator per node.
That configuration error was corrected by using native `cometbft testnet`
output, selecting each generated home before the `start` subcommand, and
assigning the matching Docker network aliases.

On the corrected run, all four RPC endpoints reported the same chain ID and a
validator set of four. Each RPC-reported node ID matched its generated node key
and peer configuration, and direct TCP probes to the peer P2P ports succeeded.
Nevertheless, every node repeatedly logged
`SecretConnection ... chacha20poly1305: message authentication failed` and
reported `numPeers=0`; all remained at height zero. A directed peer ring with
PEX disabled reproduced the failure. No common block or replicated transaction
was demonstrated. The temporary containers, volumes, network, application
processes, checkout, and logs were removed after the attempt.

The peer authentication failure remains unexplained. This is an inconclusive
multi-validator compatibility result, not evidence against the ABCI adapter or
for CometBFT safety. The single-validator transaction/query result remains
valid, but the multi-validator acceptance gate below has not passed.

## PoC Acceptance Gate

Before accepting this ADR or freezing N2:

1. Extend the successful socket PoC to a YuanLai-owned deterministic state
   machine; exercise restart, state replay, CheckTx, FinalizeBlock, Commit, and
   snapshot methods required by the release.
2. Resolve the observed SecretConnection authentication failure, then run a
   small fixed-validator local network and demonstrate authenticated peer
   connectivity, common committed blocks, a transaction queryable on every
   application instance, commit certificates, a read-only node verifying/syncing
   committed history, and deterministic state roots across honest instances.
3. Exercise validator outage, Byzantine-style conflicting proposals/votes where
   feasible, network partition, recovery, and validator-set configuration changes.
   Document that fewer than one-third Byzantine voting power is an operator and
   governance assumption, not something the software can enforce.
4. Confirm the Rust adapter is maintained and protocol-compatible, or implement
   only the required ABCI++ transport from the upstream Protobuf specification;
   review framing, bounds, concurrency, determinism, and error handling.
5. Measure installation and operation on supported user platforms. If requiring
   a second runtime/process makes the intended self-hosted product impractical,
   revisit the engine choice before any production protocol commitment.

## Consequences

- Continue N3's local-only prototype independently; do not label its output a
  chain certificate.
- Keep `spec/v2` non-normative until the PoC, validator governance, time policy,
  encoding, and cross-implementation vectors are reviewed.
- CometBFT is a research-backed candidate, not yet an accepted production
  dependency. Revisit this ADR with PoC results rather than silently freezing it.

## Sources

- [CometBFT overview and consensus model](https://docs.cosmos.network/cometbft/latest/docs/introduction/intro)
- [ABCI++ specification](https://docs.cosmos.network/cometbft/latest/spec/abci/)
- [ABCI client and server transports](https://docs.cosmos.network/cometbft/latest/spec/abci/Client-and-server)
- [CometBFT release history](https://github.com/cometbft/cometbft/releases)
- [Official CometBFT v0.40.0 arm64 container](https://hub.docker.com/r/cometbft/cometbft/tags?name=v0.40.0)
- [Rust tendermint-rs compatibility notes](https://github.com/cometbft/tendermint-rs)
- [tower-abci Rust server](https://github.com/penumbra-zone/tower-abci)
- [tower-abci v0.19.1 ABCI++ example](https://github.com/penumbra-zone/tower-abci/blob/v0.19.1/examples/kvstore_38/main.rs)
