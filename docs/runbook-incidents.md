# Incident Runbook

Use this runbook for public testnet or future mainnet incidents.

## First Actions

1. Preserve logs without exposing wallet secrets.
2. Record binary version, git commit, chain ID, node role, and height.
3. Check whether the issue is local, peer-related, or consensus-related.
4. Avoid deleting chain data until the failure is understood.

## Useful Commands

```sh
zoka status --network testnet
zoka mine-status --network testnet
zoka genesis-verify docs/genesis.testnet.v1.json
```

## Escalation

Escalate immediately if nodes disagree on finalized chain state, accept invalid
blocks, reject valid blocks, lose wallet funds, or expose private wallet data.
