# Upgrade And Hard Fork Runbook

Use this runbook for coordinated protocol upgrades.

## Planning

- Define the release tag.
- Define the activation height or timestamp.
- Publish binaries and checksums.
- Publish the exact migration or fork rule.
- Give operators enough time to upgrade.

## Execution

1. Freeze the release commit.
2. Build and verify release artifacts.
3. Publish upgrade instructions.
4. Monitor upgrade adoption.
5. Activate only when the operator threshold is reached.

## Rollback

Rollback must be explicit. Do not ask operators to downgrade unless the release
notes define a safe rollback path.
