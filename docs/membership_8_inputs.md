# Plan: Full 8/8 in-circuit membership for private_tx

## Current state

- `PRIVATE_TX_MAX_INPUTS = 8`: protocol supports up to 8 private inputs
  per tx.
- `PRIVATE_TX_MERKLE_PROVEN_INPUTS = 2`: the SNARK only proves membership
  (Merkle path against `private_commitment_root`) for **the first 2 of
  the 8 inputs**.
- The remaining 6 inputs are still validated against the on-chain tree
  by `StateManager::validate_private_transaction_membership` — but that
  check is done outside the SNARK, revealing which inputs exist in the
  tree.

## Why this matters

An observer running a full node sees that the protocol validator hit the
tree for 6 of the 8 inputs. That's a metadata leak about the tx shape:
which commitments were real vs. padding. With full in-circuit membership,
the 8 inputs are indistinguishable from zero-padding and the SNARK is
self-contained.

## Cost

Membership proof cost per input, given the current gadget:

- `PRIVATE_TX_MERKLE_MAX_DEPTH = PRIVATE_TREE_DEPTH` (16).
- Per depth: one `hash_pair_gadget` call ≈ 30 constraints (round
  constants × state updates).
- Per input: 1 leaf hash (4 word absorptions × 30 ≈ 120) + 16 depth
  steps × 30 = **≈ 600 constraints**.

Current: `PRIVATE_TX_MERKLE_PROVEN_INPUTS = 2` → ≈ 1200 membership
constraints.
Target: `PRIVATE_TX_MERKLE_PROVEN_INPUTS = 8` → ≈ 4800 constraints.

The rest of `private_tx` is ≈ 3000 constraints (range proofs, balance,
nullifiers). So full membership grows the circuit from ~4k to ~8k
constraints — roughly **2× proving time**, which is acceptable (current
proof takes a few seconds in debug, ~500ms in release).

## Changes required

1. `PRIVATE_TX_MERKLE_PROVEN_INPUTS = 8` in [groth16_backend.rs](../src/zk/groth16_backend.rs).
2. `PRIVATE_TX_PRIVATE_INPUT_WORDS` and `PRIVATE_TX_PUBLIC_INPUT_WORDS`
   recompute (already derived from the constant).
3. All call sites that build/unpack public inputs: `UserNode::send_private_transaction`,
   `StateManager::generate_private_inclusion_proofs`, etc.
4. Delete the out-of-circuit membership validator after a deprecation
   window (keep it as a sanity check for a release or two).

## Risks

- **Proving time regression**: profile before/after on realistic
  hardware. If the ~2× growth is unacceptable, fallback is to make
  `PROVEN_INPUTS` configurable per-tx and emit proofs with fewer
  verified slots when building many-input txs.
- **CRS redo**: growing public inputs invalidates the CRS — coordinate
  with the ceremony doc. The change is a purely-additive set of
  constraints, so the circuit itself stays within the same domain size
  as long as 2^⌈log2(n_constraints)⌉ doesn't cross a power-of-two.

## Accept criteria

- [ ] `PRIVATE_TX_MERKLE_PROVEN_INPUTS = 8`.
- [ ] `prop_verifier_accepts_valid_private_tx_proofs` still green.
- [ ] Benchmark: proving time on a reference machine, ≤ 2× current
  baseline.
- [ ] `StateManager::validate_private_transaction_membership` either
  deleted or demoted to `#[deprecated]` with a warning at call sites.

## Estimate

3–5 days. Mostly mechanical constant increase plus fixing the input
packing arrays. The main risk is subtle bugs in input-padding when
transitioning fewer-than-8 real inputs.
