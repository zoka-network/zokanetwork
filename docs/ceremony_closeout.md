# ZOKA Ceremony Closeout Procedure (Mainnet Blocker #5)

This document defines the final operator sequence to close the MPC ceremony and
produce release-ready artifacts.

## Inputs

- `transcript.json` with all ceremony rounds.
- Final canonical setup directory with:
  - `balance.params`
  - `merkle.params`
  - `nullifier.params`
  - `private_tx.params`
  - `DIGEST`

## One-command flow

Use:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/ceremony_closeout.ps1 `
  -Transcript C:\ceremony\zoka-mainnet-v1\transcript.json `
  -FinalDir C:\ceremony\zoka-mainnet-v1\current `
  -BundleDir C:\ceremony\zoka-mainnet-v1\public_bundle `
  -MinUniqueContributors 5 `
  -ForceBundle `
  -BuildRelease
```

This executes:

1. `verify` transcript chain against final setup.
2. `readiness` gate (threshold + digest consistency).
3. `publish-bundle` to produce public artifacts.
4. `verify-bundle` for end-to-end integrity.
5. `release-pin` to emit pinned env variables.
6. Optional release build pinned to the final digest.

## Required output artifacts

- `public_bundle/transcript.json`
- `public_bundle/readiness.json`
- `public_bundle/RELEASE.env`
- `public_bundle/setup/*.params`
- `public_bundle/setup/DIGEST`
- `public_bundle/manifest.json`
- `release.env` (or custom path via `-ReleaseEnvOut`)

## Go/No-Go gates

- `readiness.ready == true`
- `readiness.unique_contributors >= 5`
- `verify-bundle` passes with no hash mismatch.
- `release-pin` digest equals transcript tail digest.
- Release build completes with pinned digest.

## Post-closeout publication checklist

- Publish `public_bundle` in immutable storage (release asset/object storage).
- Announce final digest in at least 2 channels (website + repository release).
- Keep `transcript.json` and attestations publicly reachable.
- Keep exact release build command and commit hash in release notes.
