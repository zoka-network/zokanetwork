# Zoka 2.1.4 (Zenith testnet) — release notes

Faster mobile wallets, and a node fix that left many of their requests
unanswered.

- **Private transaction digests** (`/zoka/private-tx-digests/1`). Any node now
  serves a small public summary of each private transaction: its outputs and
  spend tags, without proofs, rings or inclusion proofs. A light wallet
  recognizes its own payments with its key and downloads the full body only of
  those, checking it against its hash before counting anything. Measured on
  Zenith: a 346 KB transaction becomes ~3 KB, and a full wallet scan goes from
  ~25 MB over Tor to a few hundred KB. Nodes learn nothing: every wallet asks
  for the same digests.
- **Fix: batch requests were dropped.** Requests for several private transaction
  bodies at once (`/zoka/private-tx-bodies/1`) were silently discarded when they
  arrived while the node was waiting for gossip, which is most of the time; the
  phone waited for its timeout and gave up. They are now answered there too.
- `GET /private/tx/:hash/digest`, for debugging: the digest plus the size of the
  body and of the digest.

**No consensus change:** same chain, same activation heights (25,570 and 25,630),
minimum client still 2.1.2. Nodes that do not update keep working, and wallets
fall back to downloading bodies when a node does not know the digests protocol.

Repositories and tags: node `zoka-network/zokanetwork` `v2.1.4`; desktop
`zoka-network/zsilent-core` `v2.1.4`; Android `zoka-network/zkapriv` `v2.1.4`.
