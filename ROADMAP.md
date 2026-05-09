# ZOKA Public Roadmap

This roadmap tracks the public testnet path and the remaining work before a
mainnet launch candidate.

## Current Release Line

- `testnet-v0.1.0`
- Public repository: `https://github.com/zokachain/zoka-network`
- Network: `zoka-testnet-v1`
- PoW: `zoka-randomx-v1`
- Token unit: ZKA, 6 decimals

## Testnet v0.1.0 Goals

- Publish a clean public source tree.
- Publish signed release assets for Windows and Linux.
- Run the first public VPS node.
- Allow users to download only `zoka` and `zokahd` binaries.
- Validate wallet creation, private transfers, mining, coinbase maturity, and
  private fee claims on a live testnet.
- Collect operator and user issues before a mainnet release candidate.

## Near-Term Work

1. Publish the GitHub Release for `testnet-v0.1.0`.
2. Build the Linux package on a Linux host or CI runner.
3. Install the Linux package on the first VPS seed node.
4. Update `docs/testnet-network.v1.json` with the public seed address.
5. Run a clean testnet mining session until coinbase maturity.
6. Repeat small private transfers with fractional fees.
7. Add at least one independent testnet node.

## Mainnet Readiness Gates

Mainnet remains blocked until all of the following are true:

- Public testnet has run with real operators for a meaningful soak period.
- Release binaries are reproducible or independently verified.
- The trusted setup and proof artifacts are reviewed for the release circuit.
- External security review has covered wallet, transaction, PoW, sync, and
  release paths.
- Final genesis, network config, signing keys, and operator roster are frozen.

## Out Of Scope For Testnet v0.1.0

- Mainnet launch.
- Exchange integrations.
- Production-value coins.
- Hard guarantees about final token economics beyond the published testnet and
  mainnet candidate artifacts.
