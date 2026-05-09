# Plan: CLSAG-pure ring signatures

## Current state

`private_wallet::mod.rs` implements a **linkable multi-input ring
signature** over Ristretto:

- Base ring of `ring_members` (one-time output public keys from decoy
  selection + the real inputs).
- Per-input key images computed as `secret_scalar · H(P_i)` (standard
  Monero pattern).
- Signature is an SoK-style Schnorr chain over a per-input response +
  one shared challenge.
- `build_ring_matrix` pads unsigned inputs with hash-derived placeholders.

It's linkable (same input spent twice → same key image → rejected) and
multi-input, but it is **not** CLSAG as defined in Monero. Differences:

- CLSAG pre-aggregates the ring keys and commitments via a `ρ` challenge
  before signing. Our code signs per-input with independent `α`
  scalars — a simpler construction but with a larger signature (N × M
  responses vs. CLSAG's N).
- CLSAG binds the amount commitment to the ring via an auxiliary
  key-image-like term. We handle amount commitments separately via
  Bulletproofs + the `amount_balance_commitment` Pedersen sum check.

The current scheme is sound and privacy-preserving, but the signature is
~M× larger than CLSAG for M-input txs and it's not wire-compatible with
Monero tooling.

## Goal

Replace the current construction with a CLSAG variant adapted to
Ristretto, matching Monero's construction byte-for-byte modulo curve
choice.

## Deliverable

- `src/private_wallet/clsag.rs` implementing:
  - `sign_clsag(ring: &[RingMember], signer_index, secret, message) -> ClsagSig`
  - `verify_clsag(ring: &[RingMember], sig, message) -> bool`
  - Output: fixed-size signature = `c_0 ‖ key_image ‖ [s_0, …, s_{N-1}]`
  - Where `RingMember` carries both the public key `P_i` and the amount
    commitment `C_i` (CLSAG signs over both).
- Delete `ring_matrix`, `sign_ring_message`, `verify_ring_message` from
  `private_wallet::mod.rs`.
- Migrate all callers.

## Reference implementation

Monero reference: `src/ringct/rctSigs.cpp` (function `CLSAG_Gen` and
`CLSAG_Ver`). The math is curve-agnostic; port it to
`curve25519_dalek::ristretto`. Useful Rust starting points:

- `monero-serai` crate — has a clean CLSAG implementation, but it's
  over Ed25519 (not Ristretto). Can be adapted.
- Zcash's `halo2-gadgets` has no CLSAG — not helpful here.

## Compatibility / migration

CLSAG signatures differ byte-for-byte from the current scheme. Every
existing private tx becomes unverifiable. Options:

1. **Hard break** (small network, easy): switch at a block height, all
   pre-switch private txs deprecated.
2. **Versioned rings**: `PrivateTransaction.ring_version: u8`, verifier
   dispatches on it. Old txs still confirm; new txs use CLSAG.

For a pre-mainnet chain, option 1 is cleaner.

## Risks

- **Crypto bugs are silent**: a broken CLSAG impl looks fine in tests
  (you signed it, you verify it) but is unforgeable only by you.
  Mitigation:
  - Cross-test against Monero's output using identical inputs + Ristretto
    basepoint substitution.
  - Run the Monero CLSAG test vectors adapted to Ristretto.
  - Get a second set of eyes before merge (critical review).
- **Performance**: CLSAG verification touches every ring member; make
  sure batch verification is on the roadmap for blocks with many tx.

## Acceptance criteria

- [ ] `ClsagSig::verify` accepts signatures it produces, for rings of
  sizes 2..16, with 1..8 real signers.
- [ ] Changing any byte of the message → verification fails.
- [ ] Changing any scalar in the signature → verification fails.
- [ ] Double-spend test: same key image in two tx → second rejected.
- [ ] Signature size: 32 + 32 + 32·N bytes for a ring of size N.
- [ ] Property test: `prop_tampered_ring_id_is_rejected` adapted.

## Estimate

2–3 weeks including cross-verification. Lower priority than ceremony
(#1), fees (#3), and public-path removal (#5) — the current scheme is
not broken, just larger and non-standard.
