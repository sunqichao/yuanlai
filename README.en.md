# YuanLai (猿来)

[中文](README.md)

> **Current direction: local-first file fingerprint registration.** The new product is at prototype stage: it can hash files with SHA-256, sign declarations, and save/verify records locally. A new blockchain, network synchronization, and public explorer have not been implemented.

## Current Product

YuanLai creates portable file-fingerprint and signed-declaration records. Source files stay on the user's device. The current web prototype signs declarations with a local Ed25519 key and stores records in a local JSON file managed by a loopback service. Registration, export, and verification work offline; browsers on the same computer can share records when opened at the same local address.

A local signature is not a blockchain record. Device time is not a trusted timestamp; a signature only shows that the corresponding key signed a record, and SHA-256 only compares file bytes. Neither proves identity, originality, or legal rights. The prototype has no internet synchronization and does not publish records automatically.

## Run the Local Prototype

Python 3 is required. From the repository root, run:

```sh
python3 web/v2-prototype/run_local.py
```

Open <http://127.0.0.1:18765/>. After registration, “本机浏览器” (Local Explorer) searches titles, declarations, record IDs, and fingerprints; filters V1/V2 records; shows signature details; exports JSON; and verifies a selected source file. The prototype limit is 500 MiB; hashing currently reads the whole file into memory, so larger files can create significant memory pressure. The service binds only to `127.0.0.1`. See the [prototype guide](web/v2-prototype/README.md) for key management and limitations.

## Roadmap and Design

- [Product proposal](docs/new-product-proposal.md)
- [Threat model](docs/new-threat-model.md)
- [Project tracking](docs/project-tracking.md)
- [V2 protocol draft](spec/v2/README.md), not frozen or implemented
- [Blockchain explorer design](docs/blockchain-explorer-design.md), a proposal for a future shared network

<details>
<summary>Legacy PoW/Credit operations guide (not the current product direction)</summary>

The earlier implementation is a PoW/Credit blockchain for course registration with Halo2 identity proofs. It remains in the repository for historical reference and separate development; it is not the new file-fingerprint registration service or a new-version blockchain.

Start the read-only project overview and open `http://127.0.0.1:18445`:

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-data --api-listen 127.0.0.1:18445
```

The page reads and validates the active chain in that datadir; API-only mode stays running. The HTTP API has no authentication or TLS and should only bind to loopback. Combine `--api-listen` with `--listen` or continuous mining to inspect a running node. See [spec/08-api.md](spec/08-api.md) for the response fields.

```sh
cargo run --release -p yuanlai-node -- --network devnet --mine --beneficiary <hex of a 32-byte public key>
```

Start a regtest peer and perform a one-shot active-chain sync from another peer:

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-a --listen 127.0.0.1:18444
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-b --connect 127.0.0.1:18444
```

`--listen` serves up to eight inbound peers concurrently. Repeat `--connect` to configure static
peers; the node tries them sequentially and exits successfully if at least one sync succeeds.
`--tx-file` transactions are submitted to each responding peer. Add `--stay-connected` to keep the
process running. CLI peers are merged into `peers.txt` in the datadir, so a restart can restore them
with only `--stay-connected`. Each peer tracks its own failures and last-success time: failures use
independent 1–30 second exponential backoff, and a successful peer is resynced after 30 seconds.
Peer attempts remain sequential short-lived TCP connections; the mode does not keep sockets open,
sync in parallel, or discover peers. Network mode cannot currently be combined with `--mine`.
The protocol and node have not had a public network security or load audit; do not expose them to
untrusted networks.

Optional transaction relay mode: the listener uses `--beneficiary` as the candidate block
beneficiary for mempool validation. A client syncs, submits each `--tx-file`, and waits for an ACK.
Newly admitted transactions are forwarded to other connected peers and atomically stored in
`mempool.dat`:

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./relay-node \
	--listen 127.0.0.1:18444 --beneficiary <hex of a 32-byte public key>
cargo run --release -p yuanlai-node -- --network regtest --datadir ./sender-node \
	--connect 127.0.0.1:18444 --tx-file ./signed-tx.bin
```

Duplicate transactions are acknowledged but not relayed again. Relay and mining cannot run
simultaneously yet. Stop the relay, then restart mining with the same datadir and beneficiary to
revalidate and load pending transactions. Network mode remains experimental and should not be
exposed to untrusted networks.

Scan a directory and generate a local manifest (this does not submit it to the blockchain):

```sh
cargo run --release -p yuanlai-cli -- scan ./my-course
```

Before the first registration, register an identity commitment for the Ed25519 public key. You can
provide a precomputed commitment:

```sh
cargo run --release -p yuanlai-cli -- identity-register \
	--network regtest --datadir ./node-data \
	--commitment <64-hex little-endian Pallas base-field commitment> \
	--key-file ./owner.key --out-dir ./identity-transactions
```

Alternatively, let the CLI compute it from separate identity-secret and salt files:

```sh
cargo run --release -p yuanlai-cli -- identity-register \
	--network regtest --datadir ./node-data \
	--identity-secret-file ./identity.secret --salt-file ./identity.salt \
	--key-file ./owner.key --out-dir ./identity-transactions
```

`identity.secret` and `identity.salt` must be independent, uniformly random canonical Pallas
base-field elements, each stored as exactly 32 raw little-endian bytes. Both files and `owner.key`
must be owner-only (`chmod 600`). Keep the identity witness separate from the Ed25519 key and back
it up securely. The command selects currently spendable Credits, writes a signed transaction file,
and prints the node command to submit it. Run that command before building the course registration
transactions:

```sh
cargo run --release -p yuanlai-cli -- register \
	--network regtest --datadir ./node-data \
	--course-dir ./my-course --title 'Course title' --version 1.0 \
	--key-file ./owner.key --out-dir ./course-transactions
```

`owner.key` must contain exactly 32 raw bytes of an Ed25519 private key. On macOS/Linux, first
run `chmod 600 ./owner.key`. The course-registration command checks the on-chain identity and
spendable Credits, writes signed transaction files, and prints the node commands to submit them
in order.

After the course is registered on-chain, generate and submit an author-verification transaction
using a challenge issued by the verifier. The verifier confirms the on-chain nullifier record
against its challenge and course root; the transaction burns 5 Credits:

```sh
cargo run --release -p yuanlai-cli -- author-verify \
	--network regtest --datadir ./node-data \
	--course-root <64-hex course root> \
	--challenge <64-hex challenge from the verifier> \
	--identity-secret-file ./identity.secret --salt-file ./identity.salt \
	--key-file ./owner.key --out-dir ./author-verifications
```

The CLI checks that the identity commitment and course root belong to the signing key, generates a
Halo2 proof and signed transaction file, then prints the node command to submit it. Challenges must
come from the verifier and should not be reused.

To register a new version, add `--parent-root <64-hex-root-of-the-latest-version>`. The directory
must still contain the full file snapshot; fees are calculated only from content not present in
the parent version.

Submit a signed, canonically encoded user transaction while mining. The node validates it against
the current chain before adding it to the next block:

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-data \
	--tx-file ./signed-tx.bin --mine --beneficiary <hex of a 32-byte public key> --blocks 1
```

## Non-financial statement

- YuanLai **issues no token** and provides no trading, exchange or pricing service.
- Registration Credits can only be spent on registrations and **expire 24 hours after they are obtained**; they are not a store of value.
- The protocol contains no price field. Selling Credits is prohibited.
- YuanLai **collects no personal data**. Identity attributes only ever enter zero-knowledge proofs computed on the user's own machine.

## What it does and does not do

| Does | Does not |
|---|---|
| Prove that content was registered before a given block height | Prove **originality** (only priority in time) |
| Prove the registrant knows the identity secret committed at registration, without revealing it | Detect re-encoded or edited copies |
| Track the version history of a course | Replace official copyright registration or notarization |

Register **before** publishing a course. See [spec/threat-model.md](spec/threat-model.md) for all known limitations.

## Core mechanisms

- **Consensus**: SHA-256 proof of work, 60-second target spacing, difficulty retarget every 60 blocks, with a difficulty floor.
- **Credits**: minted by mining and burned by registrations; transferable, but each transfer loses 10%, at most 3 hops, and every acquisition is valid for 24 hours.
- **Course write sessions**: locked Credits stop expiring while a course is being written, and are restored on cancel.
- **Zero-knowledge identity**: halo2, **no trusted setup**.
- **Author verification**: proofs bind the identity commitment, verifier challenge, and course root; nullifiers prevent reuse.

See [spec/](spec/README.md). The specification is currently written in Chinese; English translations will follow.

</details>

## Build and Check

Requires Rust (version pinned in `rust-toolchain.toml`; rustup installs it automatically).

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
./tools/check-core-deps.sh
python3 tools/testvectors/generate.py --check
```

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Report security issues **privately** as described in [SECURITY.md](SECURITY.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option. Only protocol text, explanatory material, and test vectors under `spec/` are additionally licensed under CC BY 4.0. The scope, exclusions, and link to the full legal code are in [spec/LICENSE-CC-BY-4.0.md](spec/LICENSE-CC-BY-4.0.md); other documentation, code, web assets, and trademarks are not covered by that notice.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
