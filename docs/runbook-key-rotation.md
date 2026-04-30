# ZOKA Mainnet Runbook - Validator Key Rotation

Scope: rotate validator keys without violating genesis invariants or quorum safety.

## Preconditions

- Incident ticket opened with reason (compromise, maintenance, lifecycle).
- Validator identity (`label`) and replacement keys prepared.
- New `consensus_pubkey_hex` and `operator_pubkey_hex` are distinct 32-byte ed25519 hex keys.
- At least `2f+1` active validators remain available during the maintenance window.

## Rotation policy

1. Rotate `operator_pubkey_hex` first whenever possible.
2. Rotate `consensus_pubkey_hex` in a controlled window with explicit quorum checks.
3. Never rotate more than one active validator consensus key at the same time.
4. Keep old keys revoked and archived with incident reference.

## Procedure

1. Announce maintenance window to all operators and freeze non-critical changes.
2. Update validator inventory with the new key material (out-of-band secure channel).
3. Restart the target validator process with the new consensus key.
4. Confirm the validator signs and relays PBFT traffic with the new key.
5. Confirm peers reject the old key and accept the new key.
6. Update operator key references used for administrative actions.
7. Close change with evidence bundle.

## Verification checklist

- [ ] Validator returns to healthy state within SLA.
- [ ] No PBFT quorum loss during rotation.
- [ ] New key appears in peer/operator dashboards.
- [ ] Old key is removed from active signer configuration.
- [ ] Incident/change ticket contains logs and timestamps.

## Rollback

1. Stop validator.
2. Reapply previous validated key pair.
3. Restart and rejoin consensus.
4. Re-open rotation only after root cause is cleared.
