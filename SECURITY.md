# Security policy

Zoka Quantic runs as a testnet. Testnet coins have no value, but the mainnet
will use the same software, so a bug found now protects real funds later.

## Reporting a vulnerability

**Do not open a public issue.** Write to **zokanetwork@proton.me** with:

- what is affected (component, file, version or commit);
- how to reproduce it, ideally as a test or a short program;
- what an attacker gains;
- how you would like to be credited (or that you prefer not to be).

We answer within 3 working days and keep you informed until the fix ships.
We publish a fix first and the details after, at most 90 days after your
report unless we agree on another date with you.

## Scope

What is in scope, what is not, and the properties that must hold are in
[docs/EXTERNAL-REVIEW.md](docs/EXTERNAL-REVIEW.md). In short:

- **In scope:** the transfer circuit and its proof configuration, the ledger
  and consensus rules, the node, the networking code, checkpoints, address
  formats and note encryption, the wallet engine and the wallets built on it
  (command line, exchange JSON-RPC server, ZSilent, ZKAPriv, the browser
  wallet), and release signing.
- **Out of scope:** the retired Zenith network, third-party libraries used as
  intended, attacks that need control of the victim's device, social
  engineering, and plain volume attacks.

## Severity

| Severity | Examples |
| --- | --- |
| Critical | Creating value (inflation), spending someone else's notes, a double spend that a node accepts, a proof that verifies for a false statement |
| High | Revealing who paid whom or how much in a transfer; making nodes accept a checkpoint without k valid hybrid signatures; a chain split between up-to-date nodes; a wallet paying twice or paying an address it did not check |
| Medium | Crashing or stalling nodes or wallets with small inputs; making a wallet show a wrong balance; leaking a view key or wallet file contents |
| Low | Issues that need unlikely conditions, or that leak little |

## Rewards

A bug bounty program will open before the mainnet, together with the external
review. Its rewards and rules will be published here. Reports sent before then
are credited and considered for it.

## Rules for testing

- Test on the **testnet** or on a local chain (`--network regtest`). Never
  against mainnet funds or other people's wallets.
- Do not attack the public seed nodes, relays, explorer or faucet with traffic
  floods. Run your own node to test denial-of-service ideas.
- Do not publish a vulnerability before it is fixed.

Research done in good faith under these rules is welcome. We will not take
legal action against it.

## Verifying releases

Every release is signed with a hybrid Ed25519 + ML-DSA-65 key. How to check a
download: [docs/release-verification.md](docs/release-verification.md).
