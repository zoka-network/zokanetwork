# ZOKA Ceremony Contributor Kit

This kit standardizes how each external contributor receives, executes, and
returns a ceremony round package.

## Purpose

- Keep round evidence consistent across all contributors.
- Reduce coordinator mistakes during handoff.
- Improve auditability of contribution attestations.

## Files used by this kit

- Round checklist template:
  - `docs/templates/ceremony_round_checklist.md`
- Contributor attestation template:
  - `docs/templates/ceremony_attestation_template.md`
- Contributor invitation template:
  - `docs/templates/ceremony_contributor_invitation.md`
- Round initialization helper:
  - `scripts/init_ceremony_round.ps1`
- Round export helper:
  - `scripts/export_ceremony_round_package.ps1`
- Round finalize helper:
  - `scripts/finalize_ceremony_round.ps1`
- Return ingestion + chained finalize helper:
  - `scripts/process_ceremony_round_return.ps1`
- Plan-driven return processor:
  - `scripts/process_ceremony_return_with_plan.ps1`
- Plan initializer:
  - `scripts/init_ceremony_round_plan.ps1`

## Coordinator flow per round

1. Initialize round package:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/init_ceremony_round.ps1 `
  -CeremonyDir C:\ceremony\zoka-mainnet-v1 `
  -CurrentDir C:\ceremony\zoka-mainnet-v1\current-R2 `
  -RoundNumber 3 `
  -ContributorId contrib-c `
  -Label zoka-mainnet-v1
```

2. Send to contributor:
- `round-Rk/input/*.params`
- `round-Rk/ROUND_SUMMARY.txt`
- `round-Rk/CHECKLIST.md`
- `round-Rk/ATTESTATION_contrib-<id>.md`

3. Receive from contributor:
- Modified `*.params` in `round-Rk/output_from_contributor/`
- Completed `ATTESTATION_contrib-<id>.md`

4. Finalize and append transcript:
- `zoka-ceremony-contribute finalize-adapter ... --contributor ... --note ...`

## Package export (for sharing)

```powershell
powershell -ExecutionPolicy Bypass -File scripts/export_ceremony_round_package.ps1 `
  -RoundDir C:\ceremony\zoka-mainnet-v1\round-R1 `
  -OutZip C:\ceremony\out\zoka-round-R1-contrib-a.zip
```

Outputs:
- ZIP package with `input/*.params`, checklist, summary, attestation, invitation.
- `*.sha256` companion file for integrity verification.

## Finalize after contributor return

Place returned `*.params` into `round-Rk/output_from_contributor/`, then run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/finalize_ceremony_round.ps1 `
  -CeremonyDir C:\ceremony\zoka-mainnet-v1 `
  -RoundNumber 1 `
  -ContributorId contrib-a `
  -Label zoka-mainnet-v1 `
  -InitNextRound `
  -NextContributorId contrib-b
```

Or, if contributor returns a ZIP/folder, use:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/process_ceremony_round_return.ps1 `
  -CeremonyDir C:\ceremony\zoka-mainnet-v1 `
  -RoundNumber 1 `
  -ContributorId contrib-a `
  -ReturnPath C:\returns\zoka-round-R1-contrib-a-return.zip `
  -Label zoka-mainnet-v1 `
  -InitNextRound `
  -NextContributorId contrib-b `
  -ExportNextRoundPackage
```

## Minimum contributor instructions

- Use isolated environment if possible.
- Apply high-entropy randomness during contribution.
- Do not keep copies of intermediate secret material.
- Return only required output artifacts and completed attestation.

## Required evidence retained by coordinator

- Round input hash context (`manifest.json` from final publication phase).
- Round attestation signed text (or equivalent statement).
- Transcript contribution record (`contributor`, `note`, `before`, `after`).
- Final readiness and publication verification reports.
