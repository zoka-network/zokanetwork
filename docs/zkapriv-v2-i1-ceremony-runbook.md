# ZokaPriv v2 i1 CRS Ceremony Runbook

## Purpose

This runbook prepares the final `private_tx_v2_i1.params` artifact for the
compact mobile Groth16 profile. It does not activate the profile and it does
not deploy node binaries or an APK.

The repository currently contains:

- A benchmark-only `OsRng` generator for local measurements.
- A circuit-freeze generator and independent source snapshot verifier.
- Runtime loading from an authenticated trusted-setup bundle.
- Digest pinning for node and APK builds.
- A release-candidate assembler and an independent verifier.

The repository does not contain an audited multi-party ceremony
implementation. The file under `lab-trusted-setup-v2-i1/` is unsafe for
release. Do not rename or publish it as final.

## Remaining Distance

The implementation is close to staging but not ready for public activation.
Fase 0 and Fase 1 are complete. Fase 2 runtime and its initial adversarial
matrix are complete in dormant mode. The remaining launch work is:

1. Freeze and review the exact i1 circuit revision.
2. Produce a final i1 artifact through an independently reviewed ceremony.
3. Assemble, sign and independently verify the release-candidate bundle.
4. Run coordinated BOOT, VPS2, PC miner and APK staging.
5. Restore release economics and switch the APK to self-custody.
6. Disable public `POST /private/wallet/send-from-seed`.
7. Fix activation heights only after staging passes.

## Roles

- Circuit reviewer: freezes and reviews the exact source revision.
- Ceremony operators: produce the finalized i1 artifact outside the repo.
- Release signer: signs `MANIFEST.json` offline with an RSA key.
- Independent verifier: verifies transcript, manifest, detached signature and
  bundle digest on a separate machine.
- Staging operator: uses only the independently verified package.

No one should perform every role alone.

## Inputs

- Clean source revision for the reviewed i1 circuit.
- Verified `reviewed` circuit-freeze manifest produced from that revision.
- Finalized external `private_tx_v2_i1.params`.
- SHA-256 of the archived ceremony transcript.
- Existing clean v1 `trusted-setup/` directory.
- Offline RSA signing certificate and private key held outside the repo.

Record the reviewed source revision:

```powershell
git rev-parse HEAD
git status --short
```

Do not proceed with a dirty circuit source tree for the final ceremony.
Complete
[`zkapriv-v2-i1-focused-review-checklist.md`](zkapriv-v2-i1-focused-review-checklist.md)
and verify the reviewed freeze from its exact Git checkout before generating
the final artifact.

## Assemble Candidate

Run from the repository root. Replace values with reviewed artifacts:

```powershell
.\scripts\new-zkapriv-v2-i1-release-candidate.ps1 `
  -I1ParamsPath D:\ceremony-final\private_tx_v2_i1.params `
  -CircuitFreezeManifestPath D:\reviewed\zkapriv-v2-i1-circuit-freeze.reviewed.json `
  -CircuitRevision <reviewed-git-sha> `
  -TranscriptSha256 <ceremony-transcript-sha256>
```

The script:

- Rejects `lab-trusted-setup-v2-i1/` by default.
- Verifies the existing v1 bundle against its `DIGEST`.
- Copies the four v1 params and finalized i1 params into `ceremony-output/`.
- Embeds the reviewed freeze as `CIRCUIT-FREEZE.json`.
- Recomputes the canonical runtime `DIGEST`.
- Writes `MANIFEST.json` and `MANIFEST.sha256`.

## Sign Manifest Offline

Use the release-signing workstation. Keep the private key outside the repo.
Produce a detached RSA PKCS#1 SHA-256 signature named `MANIFEST.json.sig` and
export the signer certificate as `MANIFEST.signer.cer`.

The exact signing command depends on the offline PKI. The verifier expects:

```text
MANIFEST.json.sig
MANIFEST.signer.cer
```

The certificate must contain the RSA public key corresponding to the offline
private key. Copy the detached signature and public certificate into the
candidate bundle only after signing. Publish the SHA-256 of
`MANIFEST.signer.cer` through an independent channel such as signed release
notes. Do not trust a certificate fingerprint learned only from the bundle.

## Verify Independently

On a separate machine:

```powershell
.\scripts\test-zkapriv-v2-i1-release-candidate.ps1 `
  -BundleDirectory .\ceremony-output\zkapriv-v2-i1-release-candidate `
  -RequireSignature `
  -ExpectedCertificateSha256 <out-of-band-signer-certificate-sha256>
```

The verifier checks:

- Exact allowlisted package contents.
- SHA-256 for every params file.
- Manifest SHA-256.
- Embedded circuit-freeze manifest SHA-256, status and source snapshot digest.
- Runtime-compatible canonical `DIGEST`.
- Network, profile, manifest status and canonical file order.
- Detached RSA signature when `-RequireSignature` is present.
- Signer certificate SHA-256 supplied through an independent channel.

Archive the verified directory and transcript before building staging
binaries.

## Rehearsal Only

To test the packaging workflow with the benchmark artifact:

```powershell
.\scripts\new-zkapriv-v2-i1-release-candidate.ps1 `
  -I1ParamsPath .\lab-trusted-setup-v2-i1\private_tx_v2_i1.params `
  -CircuitFreezeManifestPath .\docs\artifacts\zkapriv-v2-i1-circuit-freeze.candidate.json `
  -CircuitRevision <current-git-sha-from-freeze> `
  -TranscriptSha256 0000000000000000000000000000000000000000000000000000000000000000 `
  -OutputDirectory .\ceremony-output\unsafe-rehearsal `
  -AllowUnsafeLabArtifact

.\scripts\test-zkapriv-v2-i1-release-candidate.ps1 `
  -BundleDirectory .\ceremony-output\unsafe-rehearsal `
  -AllowUnsafeRehearsal
```

Never deploy an `unsafe-rehearsal` package.

## Build Pins

For staging node and APK builds, bind the verified digest:

```powershell
$env:ZOKA_EXPECTED_SETUP_DIGEST_COMPILED = "<verified-canonical-digest>"
```

At runtime, point nodes to the same verified directory:

```powershell
$env:ZOKA_TRUSTED_SETUP_DIR = "<verified-bundle-directory>"
```

Do not fix activation heights in the same change that introduces the final
CRS. Stage the package first with both activations still dormant.
