# Protocol

ZOKA combines a Rust full node, private wallet flows, encrypted local wallet
storage, RandomX proof-of-work, chain state validation, RPC sync, and
zero-knowledge proof infrastructure.

Important implementation areas:

| Area | Responsibility |
|---|---|
| `src/mining/` | Proof-of-work engine, difficulty, and block validation. |
| `src/state/` | Chain database, fork choice, and sync. |
| `src/network/` | Peer networking and seed connections. |
| `src/private_wallet/` | Wallet notes, claims, decryption, and private flows. |
| `src/zk/` | Circuits, setup IO, prover, and verifier. |
| `src/wallet/` | Encrypted wallet file storage. |

