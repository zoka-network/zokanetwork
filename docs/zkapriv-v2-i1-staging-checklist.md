# ZokaPriv v2 i1 Staging Checklist

## Entry Gate

- [ ] Reviewed circuit revision recorded and clean.
- [ ] Focused cryptographic review checklist closed.
- [ ] Reviewed circuit-freeze manifest verified with `-RequireCleanTree` and
      `-RequireReviewedStatus`.
- [ ] Ceremony transcript archived and SHA-256 recorded.
- [ ] Final i1 artifact did not come from `lab-trusted-setup-v2-i1/`.
- [ ] Candidate bundle assembled with
      `new-zkapriv-v2-i1-release-candidate.ps1`.
- [ ] Candidate bundle embeds the reviewed freeze as `CIRCUIT-FREEZE.json`.
- [ ] Detached manifest signature created offline.
- [ ] Signer certificate SHA-256 published through an independent channel.
- [ ] Independent machine passed
      `test-zkapriv-v2-i1-release-candidate.ps1 -RequireSignature
      -ExpectedCertificateSha256 <out-of-band-sha256>`.
- [ ] Canonical bundle digest recorded in release notes.

## Dormant Staging

- [ ] Build staging binaries pinned to the verified digest.
- [ ] Keep `PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT = None`.
- [ ] Keep `PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT = None`.
- [ ] Deploy VPS2 first.
- [ ] Confirm VPS2 boots only with the verified bundle.
- [ ] Deploy BOOT.
- [ ] Confirm BOOT and VPS2 remain synchronized.
- [ ] Start one PC miner and confirm v1 block autopush remains healthy.
- [ ] Leave `ZOKA_API_ENABLE_REMOTE_SEED_ENDPOINT` unset on staging nodes.
- [ ] Build `:app:assembleStaging`; confirm it uses
      `local-self-custody`, proof version `2` and profile
      `private_tx_v2_i1`.
- [ ] Install the APK staging build separately from the public APK
      (`com.zoka.zkapriv.staging`).
- [ ] Confirm the public APK package remains untouched.

## Simulated Activation

- [ ] Choose staging-only future heights for i1 and canonical bodies.
- [ ] Use the same heights on BOOT, VPS2 and PC miner.
- [ ] Send one local self-custody i1 payment from the APK.
- [ ] Confirm proof generation completes without UI freeze.
- [ ] Confirm recipient balance, sender deduction and change.
- [ ] Confirm miner includes the i1 body and derived nullifier set.
- [ ] Confirm both nodes accept and persist the same block.
- [ ] Confirm autopush and sync continue between nodes and PC miner.
- [ ] Restart VPS2 during a transfer and confirm recovery without fund loss.
- [ ] Confirm an old APK cannot use the disabled remote seed endpoint.

## Release Economics

- [ ] Restore `COINBASE_MATURITY_BLOCKS` from `5` to `60`.
- [ ] Restore PoW bits from `8/8/8` to `12/12/30`.
- [ ] Plan any required reset or migration before deployment.
- [ ] Re-run chain, sync, P2P, miner and APK staging after restoration.

## Public Release Gate

- [ ] APK defaults to local self-custody proving.
- [ ] Public `POST /private/wallet/send-from-seed` is disabled.
- [ ] BOOT and VPS2 use final signed CRS bundle.
- [ ] PC miner accepts and mines i1 transactions.
- [ ] External review findings are closed.
- [ ] Activation heights are fixed only after every staging check passes.
