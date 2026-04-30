# ZOKA Mainnet Runbook - PBFT Halt Recovery

Scope: recover from PBFT finality halt while preserving canonical chain integrity.

## Halt criteria

Treat as PBFT halt when both conditions hold for >= 3 block intervals:

- no finalized progress at expected cadence; and
- active validator votes observed below quorum threshold.

## Severity levels

- SEV-1: total halt with no quorum.
- SEV-2: intermittent quorum loss with delayed finality.

## Recovery workflow

1. Declare incident and freeze deployments.
2. Collect active validator liveness and vote participation.
3. Identify failing validators (network partition, key issue, process crash).
4. Restore quorum:
   - restart unhealthy validators;
   - switch to backup links (Tor/I2P or approved clearnet override);
   - if key compromise suspected, apply [runbook-key-rotation.md](runbook-key-rotation.md).
5. Validate resumed PBFT finalization over at least 10 consecutive heights.
6. Publish incident summary and action items.

## Data integrity checks

- Chain head and cumulative work are monotonic after recovery.
- No invalid reorg outside consensus rules.
- Supply and coinbase maturity accounting remain consistent.

## Escalation

- If quorum is not restored in 30 minutes, escalate to core release lead.
- If conflicting finalized views appear, stop launch and trigger governance emergency review.

## Exit criteria

- Finality restored and stable for 60 minutes.
- All validators acknowledged on-call.
- Incident report published with root cause and prevention actions.
