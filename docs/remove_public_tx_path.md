# Plan: Remove the public `Transaction` path for private txs

## Current state

Private txs travel through **two separate type lineages** that share the
public ledger:

- `private_wallet::PrivateTransaction` — the canonical, commitment-only
  type with ZK proof, nullifier, ring signature, stealth outputs.
- `transaction::Transaction` — the legacy ledger type with plaintext
  `sender`, `receiver`, `amount`. It also has an `Option<zk_proof>` and
  an optional `commitment`/`nullifier`.

When a private tx is created, the code builds a `Transaction` wrapper
with `zk_proof = Some(...)` and the plaintext fields filled in with
sender/receiver/amount — **the very data we're trying to hide**. The
REST API then redacts those fields whenever `zk_proof.is_some()`
([api/mod.rs:1778](../src/api/mod.rs#L1778), [:1853](../src/api/mod.rs#L1853),
[:2036](../src/api/mod.rs#L2036)).

Redaction at the API layer is a **second line of defense that shouldn't
exist**. The data is still in mempool memory, in storage, and in any
WebSocket subscription that forgets to apply the same redaction rule.
Anyone with a node binary (all validators) can read it.

## Goal

Private txs never get serialized into a `Transaction` with plaintext
sender/receiver/amount fields. The two types stop sharing a pipeline.

## Design sketch

### Path A — Split-pipeline (preferred)

1. `mempool::Mempool` holds two pools: `public_pool: Vec<Transaction>`
   and `private_pool: Vec<PrivateTransaction>`.
2. `block::Block.body` becomes a union `{ public_txs, private_txs }`
   rather than a single `Vec<Transaction>`.
3. Consensus, validation, state transition all fork on the variant:
   - public tx → existing path.
   - private tx → go through `validate_private_transaction`,
     `NullifierSet::spend`, state update via commitment tree.
4. API: separate `/private/tx` endpoint that returns `PrivateTransactionSummary`
   (commitment hex, tx_hash, timestamp, no amounts).
5. Storage: private txs stored under their own CF in RocksDB.

### Path B — Unified enum (less disruption)

Replace `Transaction` with `enum Tx { Public(PublicTx), Private(PrivateTx) }`.
Every code path matches on the variant. More ergonomic rust but requires
touching nearly every file in the tree.

## Changes by module

- [mempool/](../src/mempool/) — split or enum-ify.
- [block/](../src/block/) — body structure.
- [consensus/](../src/consensus/) — validate each variant.
- [state/](../src/state/) — already has `private_commitments`; just route
  private txs through it without going through the public validation
  path.
- [api/](../src/api/) — delete redaction logic; add dedicated private
  endpoints.
- [network/](../src/network/) — `PrivateTransactionMessage` already
  exists; make sure no code path wraps it in the public tx message.
- [transaction/private_tx.rs](../src/transaction/private_tx.rs) — this
  module is already `#[deprecated]`. After this refactor, delete it.

## Risks

- **Large surface area**: touches ~15 files across mempool, block,
  consensus, state, API, storage. High regression risk; extensive tests
  needed.
- **Wire format break**: existing private txs persisted to disk or
  in-flight over the network become unreadable. Need a migration step
  or a version bumper.
- **API consumers**: any external tool querying `/tx/:hash` and
  expecting redacted private txs breaks. Document the new endpoints
  clearly.

## Acceptance criteria

- [ ] `Transaction` no longer has `zk_proof`, `commitment`, `nullifier`
  fields.
- [ ] `api/mod.rs` has zero `tx.zk_proof.is_some()` branches.
- [ ] A fuzz test: any random `Transaction` accepted by the node never
  contains a commitment or ZK proof.
- [ ] Mempool + block serialization round-trips through the new split
  and is tested.

## Estimate

2–3 weeks. This is the highest-churn refactor of the five. Do it after
fees (#3) and 8/8 membership (#4) land, since both touch `PrivateTransaction`
fields and would re-conflict merges.
