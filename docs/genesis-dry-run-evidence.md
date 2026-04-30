# ZOKA Genesis Dry-Run Evidence

## Canonical file

- File: `docs/genesis.mainnet.v1.json`
- Verification command:
  - `cargo run --bin zoka -- genesis-verify docs/genesis.mainnet.v1.json --json`

## Latest local verification output

```json
{"active_voting_power":300,"chain_id":"zoka-mainnet-v1","digest_sha256_hex":"f0d9e1261a3dec08d754f63e1bcfd2663f6cedfd476657125bfaebe7aefd5745","initial_account_count":2,"path":"docs/genesis.mainnet.v1.json","pow_algorithm":"zoka-randomx-v1","pre_mine_total_zka":0,"schema":"zoka-genesis-v1","validator_count":3}
```

## Multi-host dry-run checklist

- [ ] Host 1 verified same digest.
- [ ] Host 2 verified same digest.
- [ ] Validator cohort completed pre-launch dry-run with this exact file.
- [ ] Go/No-Go record attached.
