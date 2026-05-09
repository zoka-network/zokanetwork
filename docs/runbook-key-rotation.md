# Key Rotation Runbook

Use this runbook when a node operator needs to rotate operational keys.

## Rules

- Never publish private keys.
- Rotate one operational surface at a time.
- Keep the old key available until peers have accepted the new configuration.
- Record the reason, timestamp, old public key, and new public key.

## Suggested Flow

1. Generate the new key on the target host or an offline signing host.
2. Back up the new secret material.
3. Update the relevant public configuration.
4. Restart the affected service.
5. Confirm peers can connect and sync.
6. Revoke or archive the old key according to the operator policy.
