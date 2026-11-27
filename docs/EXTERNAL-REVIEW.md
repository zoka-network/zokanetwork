# Zoka Quantic: scope for an external security review

November 2026, node 5.1.x, testnet v3. This document is for reviewers and
auditors. It says what Zoka Quantic is, which code is in scope, which
properties must hold, and which questions matter most. Every statement points
to code. When this document and the code disagree, the code is what runs, and
the disagreement is a finding.

Zoka Quantic runs as a **testnet** today. The mainnet waits for an external
review to pass (target: Q4 2027) and starts from a new genesis with the same
software. Testnet coins have no value.

An internal red-team review of testnet v3 was published on 12 November 2026
(<https://github.com/zoka-network/audit>). It was not a third-party audit.

## 1. What Zoka Quantic is

A proof-of-work coin with one shielded pool and no transparent outputs:

- **Notes** are hash commitments in a Merkle tree (depth 32). A payment spends
  notes by proving, in a STARK, that it knows their spend keys and Merkle
  paths, that their nullifiers are correct, and that values balance. There
  are no signatures in a payment: authorization is inside the proof, and the
  proof is bound to the transaction.
- **Note envelopes** are encrypted to the recipient with X-Wing (X25519 +
  ML-KEM-768) and ChaCha20-Poly1305.
- **Consensus**: RandomX (`rx/0`) over a Monero-shaped 76-byte blob, so stock
  XMRig mines it. 120-second blocks with ASERT difficulty.
- **Checkpoints**: team keys sign checkpoints with Ed25519 **and** ML-DSA-65,
  k of n (2 of 3 on the testnet). Nodes never reorganise below one.
- **Transport**: iroh (QUIC) with a required hybrid X25519MLKEM768 key exchange.

The goal: every part that protects funds or past privacy rests on hashes or
lattices, not on discrete logarithms.

## 2. In scope

| Area | What to look at | Code |
| --- | --- | --- |
| Transfer circuit (AIR) | Constraints of the v3 and v4 (narrow) circuits: ownership, Merkle membership, nullifiers, output `rho`, balance with carries, padding inputs, public values | `crates/zoka-pq-pool/src/transfer.rs`, `transfer/narrow.rs`, `transfer/narrow/` |
| Proof system configuration | Plonky3 0.8.0 (pinned): FRI over KoalaBear, Keccak for the FRI Merkle tree and the transcript, 160 queries, blowup 8, transcript domains per version | `crates/zoka-pq-pool/src/transfer.rs`, `transfer/margin.rs` |
| Hash and domains | Poseidon2 (KoalaBear, width 32) inside the circuit, domain separation (`OWNER`, `NOTE`, `NULLIFIER`, `TREE`, `HIDE`, `RHO`, `COINBASE_RHO`) | `crates/zoka-pq-pool/src/crypto.rs`, `model.rs` |
| Note model | `owner`, `hide`, `cm`, `nf`, `rho` derivation (Faerie Gold), coinbase openings | `crates/zoka-pq-pool/src/model.rs`, `ledger.rs`; [QUANTIC-V2-FORMATO.md](QUANTIC-V2-FORMATO.md) (Spanish) |
| Ledger rules | Double spends across branches, anchor window (30 blocks), coinbase maturity (60), emission, exact fee (100 atoms per input), shapes 2/4/8 → 2, 16 transactions and 32 MiB per block, proof-version activation by height | `crates/zoka-pq-pool/src/ledger.rs`, `transaction.rs` |
| Transaction format and txid | Encoding, what the txid and the proof bind, malleability | `crates/zoka-pq-pool/src/transaction.rs` |
| Keys, addresses, envelopes | Key derivation from the 24-word phrase, full / short / integrated addresses, envelope encryption and the sender-recovery path, view keys | `crates/zoka-pq-pool/src/wallet.rs`, `integrated.rs`, `scan.rs` |
| Node consensus and storage | Block validation, compact state root, branch records, reorganisation, mempool admission | `src/quantic/stark.rs` (records, ledger per branch, mempool), `stark_node.rs`, `src/state/chain.rs` |
| Proof of work | Blob construction, target comparison, ASERT | `src/mining/pow.rs`, `src/quantic/difficulty.rs`, `stark_stratum.rs` |
| Checkpoints | k-of-n hybrid signatures, which nodes obey them, rollback limits | `crates/zoka-light/src/checkpoint.rs`, `src/checkpoint_authority.rs`, `src/hybrid_signature.rs` |
| Networking | Peer protocol, message limits, bans, hybrid TLS, relays | `src/quantic/stark_net.rs`, `stark_peers.rs`, `src/network/pq_tls.rs`, `src/network/relays.rs` |
| Address directory | Publishing short addresses with an Argon2id stamp, resolution checked against the digest | `crates/zoka-light/src/directory.rs`, `src/quantic/stark_directory.rs` |
| Light wallet engine | Scanning, note selection, reservations, several pending payments, change detection, the integrated-address tag | `crates/zoka-pq-pool/src/light.rs` |
| Wallet servers and apps | The exchange JSON-RPC server, the command-line wallet, the browser wallet (WebAssembly), the phone engine | `src/quantic/stark_wallet.rs`, `stark_wallet/rpc.rs`, `crates/zoka-pq-wallet-wasm`, `src/quantic/stark_mobile.rs` |
| Release signing | Hybrid signature over `SHA256SUMS`, key handling | `src/release_signing.rs`; [release-verification.md](release-verification.md) |

## 3. Out of scope

- **Zenith**, the retired network, and everything only it uses (Bulletproofs,
  Groth16, Halo 2 / Orchard, ring signatures, team mining). That code is still
  in the repository and does not run on a Quantic network.
- **Third-party libraries used as intended**: Plonky3, RandomX, `ml-kem`,
  `x-wing`, `ml-dsa`, `ed25519-dalek`, `chacha20poly1305`, iroh, Arti.
  How Zoka uses them is in scope.
- Attacks that need control of the user's device, social engineering, and
  plain volume (spam, bandwidth floods) unless a small input causes large work.
- The hosted websites (zoka.network, zokascan.cc, wallet.zoka.network) except
  where they serve the browser wallet's code.

## 4. Properties that must hold

1. **No inflation.** No transaction or block creates value beyond the
   emission schedule (`MAX_SUPPLY` = 23,000,000 ZKA). The balance lives only in
   the proof: a soundness gap would be invisible on the chain.
2. **No theft.** Spending a note needs its spend key. Nobody can change a
   transaction in the mempool (recipient, amount, fee) without making it
   invalid.
3. **No double spend.** A nullifier is spent once on any branch, including
   across reorganisations, and two notes of one chain never share a nullifier.
4. **Privacy of past payments.** The chain does not reveal who paid whom or how
   much in a transfer, now or to an adversary that later has a quantum
   computer. Coinbase amounts are public by design.
5. **Envelope integrity.** A recipient never accepts a note whose envelope does
   not match its commitment.
6. **Wallet safety.** A wallet never pays twice for one request, never signs a
   payment it did not plan, and checks any full address a node returns for a
   short or integrated one against the digest.
7. **Checkpoint safety.** Fewer than k keys cannot make a node accept a
   checkpoint, and a single classical signature never passes.
8. **Bounded work.** A peer cannot make a node do large work (proof
   verification, storage, memory) with small or malformed inputs.

## 5. Questions we most want answered

1. Does every constraint the statement needs exist in both circuits? Can a
   witness satisfy the AIR with an unbalanced transfer, a wrong nullifier, a
   path to a different root, or a padding input that carries value?
2. Is the soundness of the configuration (160 queries, blowup 8, Keccak,
   KoalaBear with its extension) adequate, classically and against quantum
   adversaries? Our own estimate is internal and is not published as a security
   claim.
3. Do the v3 and v4 transcripts stay separate, and is the activation by height
   (v4 from block 10,000 on the testnet, v3 accepted for 100 more blocks)
   sound at the boundary and across reorganisations?
4. Can branch records and the nullifier index be confused so a side branch
   spends what the main chain spent, or the reverse?
5. Is the envelope construction (X-Wing, the associated data, the
   sender-recovery key) sound, and do view keys reveal only what they claim?
6. Can a peer or relay stall sync, split nodes, or exhaust memory?

## 6. Known limitations (not findings)

- **Checkpoints are centralised** on the team's keys while the network is
  small. They are meant to be retired as it grows.
- **Payment shape and fee are public**: 2, 4 or 8 inputs and 2 outputs, fee =
  100 atoms per input. They reveal the shape, nothing else.
- **Integrated addresses of one wallet share its short address**, so anyone can
  tell they belong to the same wallet (as with Monero's integrated addresses).
- **Block capacity**: 16 transactions per block.
- **Metadata**: relays and peers see IP addresses. Tor is optional, and Tor's
  own handshake is classical.
- **The browser wallet** is only as trustworthy as the code the site serves.
  Its WebAssembly is built from this repository; the wallet's repository
  records the commit and the SHA-256 of each file (`src/wasm/SOURCE.txt`).

## 7. Getting started

- Build: `cargo build --release --features randomx,quantic-pq --bins`.
- Pool tests: `cargo test --release --manifest-path crates/zoka-pq-pool/Cargo.toml --lib`.
- Node tests: `cargo test --release --features quantic-pq --lib quantic:: -- --test-threads=1`.
- Fuzzing: [FUZZING.md](FUZZING.md) (Spanish).
- A local chain: `--network regtest` (SHA-3 instead of RandomX, 2-block
  maturity, v4 from block 8).
- The proof v4 change: [PROOF-V4.md](PROOF-V4.md). Exchange integration:
  [EXCHANGE-INTEGRATION.md](EXCHANGE-INTEGRATION.md).

Reporting: see [SECURITY.md](../SECURITY.md).
