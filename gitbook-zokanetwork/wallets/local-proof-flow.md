# Local Proof Flow

ZOKA wallets are built around local ownership of cryptographic material. The
wallet should be the place where sensitive account data is used; public RPC
nodes should only provide chain data and accept transactions.

## ZKAPriv

In ZKAPriv, the private transaction path is implemented through the Android
application and the native `zoka_mobile` library. The wallet prepares the
private transfer locally, including the transaction body and proof-related
payload, then sends the resulting transaction to the configured RPC endpoint.

The important boundary is this:

- Recovery phrases remain local.
- Scan keys remain local.
- Spend keys remain local.
- RPC endpoints receive transaction data, not wallet secrets.
- Chain scanning uses public chain data plus local wallet material.

## ZSilent Core

ZSilent Core follows the same self-custody principle on desktop, but it does it
by orchestrating the local Zoka Core binaries. The UI does not become the owner
of private cryptography. Instead, it coordinates local wallet, node, and mining
commands through explicit service boundaries.

For wallet operations, ZSilent Core passes sensitive inputs through protected
process channels such as environment-backed password and seed variables, rather
than placing secrets directly into command-line arguments. The local binaries
then perform wallet and transaction operations on the user's machine.

## Why This Matters

This design keeps the most sensitive work close to the user:

- Wallet recovery happens from local secret material.
- Balance reconstruction happens through local scanning.
- Private send flows are prepared locally before broadcast.
- Public infrastructure does not need custody over wallet keys.

This does not remove every privacy risk. Users still need to protect their
devices, backups, network metadata, and recovery material. The goal is to avoid
turning RPC servers, explorers, websites, or support channels into places where
private wallet data is exposed.
