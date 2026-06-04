# ZokaPriv v2 i1 Focused Cryptographic Review

## Scope

This checklist reviews the exact `private_tx_v2_i1` source snapshot before a
final Groth16 ceremony. It is not a substitute for an independent audit.

Start by generating and verifying a review-candidate freeze:

```powershell
.\scripts\new-zkapriv-v2-i1-circuit-freeze.ps1 -Force
.\scripts\test-zkapriv-v2-i1-circuit-freeze.ps1
```

The candidate may describe a dirty development workspace. A ceremony-ready
freeze must be regenerated from a clean revision with `-Status reviewed`,
`-Reviewer` and the SHA-256 of an archived external review record.

## Circuit Statement

- [ ] Confirm profile shape: one input, one Merkle path, depth `32`, four
      scalar words, `173` private input words and `12` public input words.
- [ ] Review every allocated witness and public input in
      `src/zk/groth16_backend.rs`.
- [ ] Confirm inactive padding cannot bypass value, asset, commitment or
      membership constraints.
- [ ] Confirm Merkle direction bits are boolean and the reconstructed root is
      constrained to the public anchor.
- [ ] Confirm tree hashing and round constants match between host state and
      circuit gadgets.
- [ ] Confirm proof serialization remains `192` bytes.

## Transaction Composition

- [ ] Confirm the body hash is domain-separated for proof version `2` and
      profile `private_tx_v2_i1`.
- [ ] Confirm the signed CLSAG message commits to the intended v2 body hash.
- [ ] Confirm Bulletproof ranges, asset conservation, fee and change rules are
      still enforced for i1.
- [ ] Confirm nullifiers are derived, committed and compared exactly once.
- [ ] Confirm false roots, altered proofs, duplicated hashes, duplicated
      nullifiers, downgrade metadata and missing bodies are rejected.

## Protocol Integration

- [ ] Confirm i1 remains dormant unless both activation heights are set.
- [ ] Confirm nodes load i1 params only from an authenticated setup bundle.
- [ ] Confirm P2P, mempool, mining, canonical block bodies and sync all use the
      same profile policy and candidate block height.
- [ ] Confirm APK local proving keeps seed and spending key on-device.
- [ ] Confirm the public remote `send-from-seed` path is disabled before
      release.

## Freeze Promotion

Archive the review record outside the repository and compute its SHA-256.
From a clean reviewed revision:

```powershell
.\scripts\new-zkapriv-v2-i1-circuit-freeze.ps1 `
  -OutputPath D:\reviewed\zkapriv-v2-i1-circuit-freeze.reviewed.json `
  -Status reviewed `
  -Reviewer <reviewer-id> `
  -ReviewRecordSha256 <external-review-record-sha256>

.\scripts\test-zkapriv-v2-i1-circuit-freeze.ps1 `
  -ManifestPath D:\reviewed\zkapriv-v2-i1-circuit-freeze.reviewed.json `
  -RequireCleanTree `
  -RequireReviewedStatus
```

Only a verified `reviewed` freeze may be paired with a final ceremony
artifact.
