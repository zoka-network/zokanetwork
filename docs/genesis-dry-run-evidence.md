# Genesis Dry-Run Evidence

This document records the digest expected by the regression test for the
published mainnet candidate genesis file.

## Canonical File

```text
docs/genesis.mainnet.v1.json
```

## Verification Command

```sh
cargo run --bin zoka -- genesis-verify docs/genesis.mainnet.v1.json --json
```

## Latest Verification Output

```json
{"active_voting_power":300,"chain_id":"zoka-mainnet-v1","digest_sha256_hex":"f0d9e1261a3dec08d754f63e1bcfd2663f6cedfd476657125bfaebe7aefd5745","initial_account_count":2,"path":"docs/genesis.mainnet.v1.json","pow_algorithm":"zoka-randomx-v1","pre_mine_total_zka":0,"schema":"zoka-genesis-v1","validator_count":3}
```

## Rule

If the mainnet candidate genesis changes, update the JSON artifact and this
evidence document in the same commit.
