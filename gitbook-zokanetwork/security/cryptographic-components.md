# Cryptographic Components

ZOKA's codebase includes:

- Ristretto255-based private wallet operations.
- BIP39 mnemonic-based recovery flows.
- Commitments and nullifiers for private transfer accounting.
- zk-SNARK proof infrastructure.
- Bulletproof-style range proof components for private value constraints.
- Encrypted wallet storage.

Implementation details can change across releases, so integration work should
track the active release branch and protocol activation heights.

## How They Fit Together

Commitments let the chain account for private values without publishing the
plain amount. Nullifiers prevent double spends by making spent notes detectable
without revealing the original note owner. zk-SNARK verification lets a node
check private transaction validity against the active circuit rules.

Wallet recovery and scanning are separate concerns. A recovery phrase restores
wallet identity, while scan material lets the wallet detect relevant encrypted
outputs in chain data. Neither belongs in a public explorer query or support
ticket.

## Release-Critical Material

Trusted setup or CRS files, wallet derivation code, proof verification code,
and activation-height logic must be treated as release-critical. Development
setup paths and seeded test setup must not be reused for production-value
networks.
