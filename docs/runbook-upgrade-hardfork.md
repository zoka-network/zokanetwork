# ZOKA Mainnet Runbook - Upgrade and Hard Fork

## Scope

Safe rollout of protocol upgrades and emergency hard forks.

## Preconditions

- Release artifacts reproducible and signed.
- Upgrade notes published with rollback instructions.
- Validator/operator acknowledgment >= 80% before cutover.

## Planned upgrade process

1. Publish release candidate, hashes, and signature verification instructions.
2. Run testnet replay with target build.
3. Confirm validator readiness and maintenance window.
4. Deploy staged rollout:
   - non-validator nodes first;
   - validators second;
   - edge/public RPC last.
5. Monitor chain health for at least 2 halving windows worth of blocks or agreed launch SLO.

## Emergency hard fork process

1. Trigger SEV-1 governance response.
2. Freeze all non-essential changes.
3. Publish minimum-change patch and fork activation condition.
4. Collect validator/operator signatures for activation approval.
5. Execute synchronized rollout and verify post-fork chain state.

## Rollback

- If consensus divergence occurs before activation point, roll back to previous signed release.
- If divergence occurs after activation, follow PBFT halt recovery and governance emergency process.

## Exit criteria

- No consensus divergence.
- Stable block production/finality.
- Public post-upgrade report with observed metrics and known issues.
