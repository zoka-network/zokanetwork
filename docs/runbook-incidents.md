# ZOKA Mainnet Runbook - Incident Response

## Purpose

Standard incident handling for mainnet launch and early production operation.

## Roles

- Incident Commander (IC)
- Communications Lead
- Core Dev Lead
- Ops Lead

## Severity matrix

- SEV-1: chain halt, consensus safety risk, critical exploit.
- SEV-2: degraded sync/finality, partial outage.
- SEV-3: non-critical bug with workaround.

## Response process

1. Detect and classify severity.
2. Assign IC and open incident channel.
3. Freeze risky changes and branch cuts.
4. Mitigate user impact.
5. Confirm technical recovery.
6. Publish external/internal postmortem.

## Mandatory artifacts

- Timeline with UTC timestamps.
- Impact assessment and affected components.
- Root cause statement.
- Corrective actions with owners/dates.

## Launch-day additions

- Keep SEV-1 bridge active for the full launch window.
- Require explicit go/no-go sign-off before resuming planned operations.
