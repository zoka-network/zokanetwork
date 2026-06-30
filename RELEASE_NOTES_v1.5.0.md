# Zoka / ZSilent 1.5.0 Mainnet Hotfix

Release type: mandatory mainnet hotfix.

## Why This Release Exists

- Migrates mainnet economics to atomic accounting and rejects mixed full-ZKA/atom ledgers.
- Restores private transaction mining and reward accounting.
- Anchors the mainnet recovery branch with checkpoints at heights 296, 297, and 298.
- Rejects old/off-branch miners that do not follow the canonical recovery chain.
- Requires miner heartbeat client version 1.5.0 or newer for ZSilent/ZokaScan active-miner reporting.
- Adds HTTP block autopush/retry for NAT'd miners through `/sync/block`.
- Keeps `/sync/block` enabled on public RPC/bootstrap nodes with stricter API limits.

## Mainnet Recovery Checkpoints

- Height 296: `d8c4b8d5945469c46fb32c002d34c08cb736ef470578f2464c3054ac190ac95c`
- Height 297: `a086b823ec51052473860211327bc7e5bad9262cb68df264ecafc05ee070e22f`
- Height 298: `bee49728c38951fc82dfda00d2b4000980cb7a3b07046d6f6c636bd1f2d98227`

Nodes running 1.5.0 reject any mainnet branch above these heights that does not contain the exact hashes above.

## Difficulty Recovery Window

- Recovery cap starts at height 297.
- Recovery cap ends at height 360.
- Maximum capped difficulty during the window: 14 bits.
- After height 360, normal retargeting resumes. The live network was observed past height 360 with difficulty increasing again.

## Validation Performed

- Windows release build: `cargo build --release --features randomx --bin zoka`
- Linux release artifact built by GitHub Actions from commit `262a4473e46f6a01ffa74f2e5f1236187dc42598`.
- Mainnet observed past height 360 with local, bootstrap, and RPC nodes on the same canonical hashes.
- Private transaction test confirmed in block 318:
  - Tx hash: `94fec103baa98263524a278cc0912c21`
  - Block hash: `f548c56f38d0c604a0a8f4592020c5e9c7260f4cff7a0df81108453ef399c7f9`
- Miner heartbeat reporting verified:
  - `miners_online = 1`
  - `legacy_miners_online = 0`
  - `outdated_miners_online = 0`
  - `minimum_client_version = 1.5.0`
- Bootstrap/RPC health endpoints reported `healthy`.

## Checksums

- Windows `zoka.exe`: `88AE5F6DC8E44CD13EDE14109F33FC4122785FEE34902E087EAA145B22607C6E`
- Windows `zokahd.exe`: `0A22596A39984BE4BDAF0DFB80C19906E1614DFA785D1B25E5E0B4FFEF7BA39F`
- Linux `zoka`: pending final 1.5.0 build

## Operator Notes

- Upgrade all public nodes and miners to 1.5.0.
- Do not reuse a pre-hotfix local chain branch that diverges after height 296.
- Nodes should sync from the canonical branch containing checkpoints 296, 297, and 298 above.
- Public bootstrap/RPC nodes should keep `ZOKA_API_ENABLE_BLOCK_SUBMIT=true` so local/NAT'd miners can autopush valid mined blocks.
- Recommended public API limits:
  - `ZOKA_API_RATE_LIMIT=180`
  - `ZOKA_API_READ_RATE_LIMIT=5000`
  - `ZOKA_API_TX_RATE_LIMIT=120`
  - `ZOKA_API_MAX_BODY_BYTES=1048576`

## ZSilent Notes

- ZSilent Core 1.5.0 must package the 1.5.0 `zoka.exe` binary.
- ZSilent and explorer active-miner views intentionally ignore old miner clients below 1.5.0.
- Coinbase rewards still require 60 confirmations before spend.
