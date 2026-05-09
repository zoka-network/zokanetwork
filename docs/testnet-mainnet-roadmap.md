# ZOKA Testnet -> Mainnet Roadmap

Status: draft for local/public testnet preparation

## Principle

Testnet is not a fork of product logic. It is a separate network profile that
uses the same blockchain engine as mainnet:

- same private transaction format
- same private fee claims
- same atomic ZKA accounting
- same RandomX PoW path
- same block validation and sync code
- separate chain id, genesis, DBs, wallets, seeds and release notes

Any blockchain, wallet privacy, PoW, fee, sync, serialization or economics bug
found on testnet must be fixed in shared code first. Mainnet inherits the fix
immediately through the shared engine; testnet-only patches are allowed only for
operator config, seeds, faucet and documentation.

## Network Profiles

Mainnet:

- chain id: `zoka-mainnet-v1`
- local DB: `.zoka/mainnet/blockchain.db`
- local wallets: `.zoka/mainnet/private_wallets`
- local mining state: `.zoka/mainnet/user_mining_state.json`
- local logs: `.zoka/mainnet/logs`
- config: `docs/mainnet-network.v1.json`
- genesis: `docs/genesis.mainnet.v1.json`
- economics artifact: `docs/economics.mainnet.v2-atoms.json`

Testnet:

- chain id: `zoka-testnet-v1`
- local DB: `.zoka/testnet/blockchain.db`
- local wallets: `.zoka/testnet/private_wallets`
- local mining state: `.zoka/testnet/user_mining_state.json`
- local logs: `.zoka/testnet/logs`
- config: `docs/testnet-network.v1.json`
- genesis: `docs/genesis.testnet.v1.json`
- economics artifact: `docs/economics.testnet.v1.json`

## Phase 1: Local Testnet Profile

- Add `zoka-testnet-v1` as a first-class chain id.
- Keep testnet PoW on `zoka-randomx-v1`.
- Use separate local directories for DB, logs and wallets.
- Verify genesis, mining, maturity, private sends and private fee claims.
- Keep old local rehearsal artifacts out of the active `.zoka` root.

Exit criteria:

- fresh testnet DB starts at height 0;
- miner reaches maturity;
- `0.000001 ZKA` private send with `0.0001 ZKA` private fee passes audit;
- block includes `private_fee_claim_count >= 1`;
- same tests still pass for mainnet code paths.

## Phase 2: Public Testnet

- Publish 2-3 seed nodes with independent operators.
- Publish a signed testnet genesis digest.
- Add public RPC endpoints with privacy-safe logging policy.
- Add faucet or guided mining instructions for early testers.
- Run a public explorer that does not reveal address history.

Exit criteria:

- external nodes can sync from zero;
- explorer confirms blocks and tx hashes only;
- wallets can scan/decrypt their own history;
- testnet incident log is maintained.

## Phase 3: Mainnet Freeze

- Replay all testnet bug fixes on shared code.
- Re-run mainnet genesis verification.
- Re-run release build with RandomX and setup digest checks.
- Sign release artifacts.
- Freeze operator roster and seed endpoints.

Exit criteria:

- no open consensus/privacy/Pow/wallet blockers;
- external audit issues triaged;
- public launch checklist passes without testnet-only exceptions.

## Non-Negotiables

- Do not weaken privacy in testnet for convenience.
- Do not add public address history to the explorer.
- Do not make testnet use a weaker consensus path than mainnet.
- Do not patch testnet-only blockchain bugs; fix shared code.
- Do not reuse mainnet DB or wallet directories for testnet.
