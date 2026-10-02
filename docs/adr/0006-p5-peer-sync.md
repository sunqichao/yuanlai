# ADR 0006: P5 Peer Sync and Active-Chain Reorganization

- Status: accepted
- Date: 2026-10-01

## Context

P0 already provides a validated `HeaderTree` with cumulative-work fork choice, while `Chain` has
reversible per-block deltas. P5 needs to exchange headers and blocks without treating peer data as
trusted state, activate heavier branches atomically, and preserve a replayable active chain after a
restart.

## Decisions

1. **Transport:** use TCP from the standard library. One node invocation runs either a sequential
   peer listener or a one-shot sync client; network mode does not mine concurrently in this
   iteration.
2. **Framing:** use `YLP2 ‖ payload_len:u32_le ‖ payload`, with the canonical P2P message codec and
   a maximum payload of `MAX_BLOCK_SIZE + 1`. Check the declared length before allocating payload
   memory. Chain identity is checked in the `Version` handshake.
3. **Handshake:** require protocol version 1, matching `chain_id`, a nonzero peer nonce, and one
   `Verack` from each side before processing requests. The OS CSPRNG supplies per-connection nonces;
   failure to obtain one aborts the connection.
4. **Synchronization:** fetch bounded header pages, validate all headers through `HeaderTree`, then
   request block bodies by ID. A block is accepted only when its header exactly matches the
   validated header and full consensus validation succeeds.
5. **Fork activation:** `Chain::reorganize` works on a clone, disconnects to the common ancestor,
   and applies the complete replacement branch before committing in memory. The active block file
   is rewritten through a synced sibling temporary file and rename. Side branches are not persisted
   and may be downloaded again after restart.
6. **Security scope:** P2P is unauthenticated and unencrypted. The listener is sequential and has
   timeouts, but peer quotas, per-command rate limits, transaction relay, discovery, and simultaneous
   mining are deferred. Do not expose this iteration to untrusted public networks.

## Consequences

- `spec/07-p2p.md` is normative for frame layout, message tags, handshake and synchronization.
- Header and body data received over the network never bypasses consensus validation.
- Each successful sync persists exactly one linear active chain; restart replays and revalidates it.
- `rand` is a runtime dependency in the CLI and node because proof and connection nonces require
  operating-system randomness.