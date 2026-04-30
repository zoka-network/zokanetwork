# ZOKA External Cryptography Audit Prep (Mainnet Blocker #9)

This document defines the pre-audit package required before contacting external
auditors.

## Objective

Deliver a reproducible, scoped, and frozen code snapshot so auditors can review
soundness and security-critical assumptions without moving targets.

## Mandatory pre-audit gates

1. Ceremony blocker (#5) completed and published.
2. Release digest pin finalized and reproducible.
3. Code freeze declared for audit scope modules.
4. Test and verification baseline attached.

## Audit scope (minimum)

- `src/private_wallet/*` (CLSAG, private transaction construction/verification)
- `src/zk/*` (circuits, proving/verifying, setup loading, digest checks)
- `src/network/*` (protocol freeze, private message validation, mixer path)
- `src/state/*` and `src/storage/*` (consensus-critical persistence invariants)
- `src/main.rs` operational paths affecting setup loading and node bootstrap

## Out of scope (unless auditor requests)

- UI/UX copy and non-consensus CLI cosmetics
- Historical legacy code already unreachable from runtime paths

## Deliverables to auditor

- Commit hash and branch/tag under audit.
- `ROADMAP.md` and `PENDIENTE.md` snapshots.
- Ceremony artifacts and verification docs:
  - `docs/ceremony_mpc.md`
  - `docs/ceremony_runbook.md`
  - `docs/ceremony_closeout.md`
  - `docs/ceremony_contributor_kit.md`
- Build and validation scripts:
  - `scripts/build_release_with_setup_digest.ps1`
  - `scripts/publish_ceremony_bundle.ps1`
  - `scripts/verify_ceremony_bundle.ps1`
  - `scripts/ceremony_closeout.ps1`
  - `scripts/init_ceremony_round.ps1`
- Reproducibility evidence:
  - `cargo check` output
  - target release build command
  - digest pin material (`RELEASE.env` or `release.env`)

## Code-freeze checklist

- [ ] Freeze date/time announced.
- [ ] Audit branch/tag created.
- [ ] No pending protocol or cryptographic changes in freeze window.
- [ ] CI green on frozen revision.
- [ ] Working tree clean for release artifacts.

## Budget and vendor checklist

- [ ] At least 2 audit proposals requested.
- [ ] Scope and assumptions aligned in writing.
- [ ] Delivery format agreed (findings severities + remediation expectations).
- [ ] Re-audit or follow-up window agreed for high/critical issues.

## Remediation flow

1. Triage findings by severity.
2. Patch and test each finding in isolated commits.
3. Re-run critical invariants and ceremony/digest gates.
4. Publish remediation report with before/after references.

## Package generation helper

Use:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/prepare_audit_package.ps1 `
  -OutDir C:\audit\zoka-preaudit `
  -IncludeCargoCheck
```

This generates a shareable package with docs/scripts snapshots, git metadata,
and optional `cargo check` log.
