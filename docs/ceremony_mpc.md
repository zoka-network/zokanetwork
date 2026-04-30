# Plan: Real MPC trusted-setup ceremony

## Why this is blocking

ZOKA's Groth16 proofs are only sound if the toxic waste τ used to generate the
CRS is unknown to every participant. The current `zoka-trusted-setup` binary
runs on a single machine with `OsRng`; τ exists in that process's memory for
a few seconds before being dropped. A malicious operator could instrument
`OsRng`, patch `bellman`, or snapshot VM memory to exfiltrate τ and later
**forge arbitrary `private_tx` proofs** undetectably. Zero-knowledge (the
privacy property) is not affected — only soundness (the "no fake money"
property) is.

The cure is a multi-party ceremony where τ is split across N independent
contributors. As long as at least one contributor destroys their share, τ is
unrecoverable.

## Deliverable

A reproducible pipeline that:

1. Takes a well-known Phase 1 output (Perpetual Powers of Tau, `ppot_0080_28.ptau`
   or larger).
2. Runs a Phase 2 contribution loop for each of the 4 circuits
   (`balance_circuit`, `merkle_circuit`, `nullifier_circuit`, `private_tx`).
3. Produces `<circuit>.params` files compatible with
   `zk::setup_io::read_circuit_params`.
4. Produces a canonical `DIGEST` file and a contributor transcript/attestation
   log.

## Recommended tooling

Two realistic paths:

### Path A — snarkjs / circom bridge (1–2 weeks)

- Rewrite the 4 circuits in Circom.
- Use `snarkjs groth16 setup` + `snarkjs contribute` for Phase 2.
- Export `*.zkey` → convert to bellman-compatible `Parameters<Bls12>` via a
  small Rust adapter (`bellman::groth16::Parameters::read` expects a specific
  serialization that must be matched byte-for-byte).

**Pros:** mature tooling, well-documented ceremony UX, browser-based
contribution clients exist.
**Cons:** porting the `private_tx` circuit (≈1200 lines of bellman gadget
code) to Circom is a non-trivial cryptographic rewrite; risk of introducing
constraint mismatches.

### Path B — native bellman phase2 crate (2–4 weeks)

- Integrate [`phase2`](https://github.com/ebfull/phase2) (the crate used by
  original Zcash Sapling ceremony).
- Write a `zoka-ceremony-contribute` binary that:
  - Accepts an input params file + entropy from the participant.
  - Produces an output params file + SHA-256 of the new state.
  - Zeroizes its memory before exit.
- Coordinate N contributors via a simple GitHub repo (each PR is one
  contribution, reviewers verify the hash chain).

**Pros:** circuits don't change; the output is already in the format
`setup_io` reads.
**Cons:** `phase2` crate is lightly maintained, requires careful review.

## Integration hooks already in place

- `src/zk/setup_io.rs` reads `Parameters<Bls12>` from disk + validates
  a `DIGEST`. No code changes needed in the node to consume ceremony output.
- `ZOKA_EXPECTED_SETUP_DIGEST_COMPILED` lets the release binary pin
  itself to the ceremony output at build time. Bake this once the
  ceremony is done and ship.
- `ZOKA_TRUSTED_SETUP_DIR` env var points the runtime at the bundle.

## Acceptance criteria

- [ ] ≥5 independent contributors, ≥2 on different hardware classes
  (e.g., air-gapped laptop + cloud VM + physical HSM).
- [ ] Public transcript of contributions + attestations.
- [ ] Final `DIGEST` published in a git tag + announced out-of-band (blog,
  forum).
- [ ] Release binary built with `ZOKA_EXPECTED_SETUP_DIGEST_COMPILED` set.
- [ ] `target/release/zoka` refuses to boot against any bundle that
  doesn't match the compiled digest (smoke test in CI).

## Operational commands (implemented)

- `zoka-ceremony-contribute readiness --transcript <path> --final-dir <dir> --min-unique-contributors 5 --out-json <path>`
  validates threshold + digest chain and emits auditable JSON.
- `zoka-ceremony-contribute publish-bundle --transcript <path> --final-dir <dir> --out-dir <dir> --min-unique-contributors 5 [--force]`
  creates a public bundle with `transcript.json`, `readiness.json`,
  `RELEASE.env`, canonical `setup/*.params + DIGEST`, and `manifest.json`
  (sha256 for every published artifact).
- `zoka-ceremony-contribute verify-bundle --bundle-dir <dir>`
  verifies publication bundle integrity end-to-end (`manifest` hashes,
  readiness consistency, transcript tail, and canonical setup digest).
- Helper script: `scripts/publish_ceremony_bundle.ps1`.
- Helper script: `scripts/verify_ceremony_bundle.ps1`.
- Runbook operativo completo (5 contribuidores): `docs/ceremony_runbook.md`.

## Risks

- **Bugs in the Phase 2 adapter** that preserve the "looks like a valid CRS"
  property but break soundness. Mitigation: after ceremony, run every
  existing test against the new params — circuits should prove and verify
  exactly as before. Any regression is evidence the conversion is wrong.
- **Contributor collusion**: at least one honest, non-colluding participant
  is the security assumption. Diversify geography + organizations.
- **Discoverable old CRS artifacts**: before launch, grep the repo and all
  forks for the dev seeded params (`seeded_rng`) and delete every copy.

## Estimate

2–4 weeks of focused work (1 engineer, full-time) + coordination time
with contributors. Not parallelizable with the `private_tx` circuit
changes — ceremony must be redone if the circuit changes.

## Until the ceremony lands

Every deployed node today operates under **trust-on-first-setup** of
whoever ran `zoka-trusted-setup`. That is safer than the seeded fallback
(τ is not in source), but weaker than MPC (τ existed in one process). This
is fine for devnets and testnets; it is not fine for a network backing
real value.
