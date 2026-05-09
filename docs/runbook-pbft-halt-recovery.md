# PBFT Halt Recovery Runbook

The PBFT module is present in the codebase, but public testnet v0.1.0 primarily
validates the PoW chain path. Keep this runbook for future validator-set
testing.

## Symptoms

- Validator votes stop progressing.
- Nodes disagree about quorum state.
- Chain data continues but finality state does not advance.

## Recovery Outline

1. Stop adding new validator changes.
2. Capture logs and node heights from every validator.
3. Identify whether the halt is connectivity, key, clock, or software related.
4. Restart only after preserving evidence.
5. Resume with the smallest validator set change possible.
