# Zoka Quantic — Specification for External Review

Draft of 17 October 2026, branch `etapa5-quantic-nodo` of the node repository.
Zoka Quantic is not live; no public network, genesis or release exists yet. This
document describes what ZOKA built around Zcash's Ironwood construction, so that
reviewers can check it without reverse-engineering the code. Every statement here
points to code; when the two disagree, the code is what runs and the
disagreement is a finding.

## 1. Scope

**Reused without modification (out of scope unless its use is wrong):**

- `orchard` 0.16.0 with `BundleVersion::ironwood_v3()`: the action circuit,
  Halo 2 proving and verification (`halo2_proofs` 0.4), RedPallas, Sinsemilla
  note commitments, Poseidon nullifier PRF, value commitments, ZIP 32 Orchard key
  derivation, V3 ("quantum-recoverable", ZIP 2005) note plaintexts, the PCZT
  roles used to assemble bundles, and `BatchValidator`.
- RustCrypto `ml-kem` 0.3.2, `x-wing` 0.1.1 and `ml-dsa` 0.1.1 behind ZOKA's
  wrapper crate `crates/zoka-pq` (tested against the official X-Wing draft-10 and
  NIST vectors).

**New, in scope:**

| Area | Section | Code |
|---|---|---|
| Key derivation and the X-Wing key | 3 | `crates/zoka-quantic/src/keys.rs` |
| Address format | 4 | `crates/zoka-quantic/src/address.rs` |
| Note envelope (replaces Orchard note encryption) | 5 | `crates/zoka-quantic/src/note_encryption.rs` |
| Transaction wire format, txid and authorization digest | 6 | `crates/zoka-quantic/src/tx.rs` |
| Consensus rules | 7 | `crates/zoka-quantic/src/state.rs`, `src/quantic/mod.rs` |
| Light-wallet compact format and its verification | 8 | `crates/zoka-quantic/src/compact.rs`, `src/quantic/wallet_sync.rs` |
| Difficulty and chain selection | 9 | `src/quantic/difficulty.rs`, `src/state/chain.rs` |
| Hybrid Ed25519 + ML-DSA-65 signatures | 10 | `src/hybrid_signature.rs`, `src/checkpoint_authority.rs`, `src/snapshot.rs` |

The hypotheses we most need checked are collected in section 11.

## 2. Notation

- `H256_p(x)`, `H512_p(x)`: BLAKE2b with 32- or 64-byte output and the 16-byte
  personalization `p`; `x` is the concatenation of its parts. The
  personalizations are listed below and are pairwise distinct (tested).
- `‖`: concatenation. Integers are little endian unless stated.
- X-Wing: `draft-connolly-cfrg-xwing-kem-10`. `Encaps_derand(pk, eseed)` takes 64
  bytes of encapsulation randomness; `KeyGen(seed)` takes a 32-byte seed.
- AEAD: ChaCha20-Poly1305 (RFC 8439), all-zero 96-bit nonce, empty associated
  data.

| Personalization | Use |
|---|---|
| `ZokaQuanticSeed_` | wallet root → ZIP 32 seed |
| `ZokaQ_XWingSeed_` | Orchard spending key → X-Wing seed |
| `ZokaQ_AddrCheck_` | address checksum |
| `ZokaQ_KemEncSeed` | note → X-Wing encapsulation randomness |
| `ZokaQ_NoteEncKey` | X-Wing shared secret → note key |
| `ZokaQ_OutCiphKey` | outgoing viewing key → outgoing cipher key |
| `ZokaQ_TxHeader__`, `ZokaQ_ActCompact`, `ZokaQ_ActMemos__`, `ZokaQ_ActNonComp`, `ZokaQ_BundleEff_`, `ZokaQ_TxId______`, `ZokaQ_TxAuth____` | transaction digests (section 6) |
| `ZokaQ_MerkleNode` | per-block transaction trees |

## 3. Keys

```text
root        16..64 bytes (the BIP-39 entropy of the wallet's phrase)
seed        = H512_"ZokaQuanticSeed_"(root)
sk          = Orchard spending key at m/32'/coin_type'/account'     (ZIP 32)
xwing_seed  = H256_"ZokaQ_XWingSeed_"(sk)
(xwing_sk, xwing_pk) = X-Wing.KeyGen(xwing_seed)
```

- `coin_type` is `0x5a4b` on mainnet and 1 on the test networks.
- Every Orchard key (`ask`, `nk`, `rivk`, `ivk`, `ovk`, diversified addresses)
  is the crate's own.
- The X-Wing key takes the role of the incoming viewing key: it is what lets a
  wallet recognize its payments.
- **One address per wallet**: the Orchard external address at diversifier index
  0, plus the X-Wing key. Diversified addresses sharing one X-Wing key would be
  linkable through it, so wallets do not produce them. Change goes to the
  Orchard internal address, which is never shown.
- Exportable viewing key, 129 bytes: network id (1) ‖ Orchard full viewing key
  (96) ‖ `xwing_seed` (32).

## 4. Addresses

```text
content  = 0x01 ‖ Orchard raw address (43) ‖ xwing_pk (1,216)
checksum = first 4 bytes of H256_"ZokaQ_AddrCheck_"(prefix ‖ content)
address  = prefix ‖ Base58(F4Jumble(content ‖ checksum))          about 1,730 characters
```

- Prefixes, not final: `zq1` (mainnet), `zqtest1`, `zqregtest1`.
- Decoding is strict: known prefix for the expected network, Base58, exact length
  (1,264 bytes), F4Jumble⁻¹ (ZIP 316), checksum, version byte, a valid Orchard
  address and a parseable X-Wing key.
- **Purpose of F4Jumble:** without it the visible beginning of the string would
  be the Orchard part. An attacker could keep it and substitute their own X-Wing
  key; a payer who compares only the beginning would then send a note its owner
  can never find. With F4Jumble every character depends on every byte.

## 5. Note envelope

Each action carries, instead of Orchard's `epk`/`enc_ciphertext`/`out_ciphertext`,
an envelope `(kem, enc, out)`. The action's `epk` field, which the circuit does
not constrain, is set to a fixed placeholder (the Pallas generator).

```text
eseed     = H512_"ZokaQ_KemEncSeed"(rseed ‖ rho ‖ cmx)
(kem, ss) = X-Wing.Encaps_derand(recipient xwing_pk, eseed)                1,120 bytes
key       = H256_"ZokaQ_NoteEncKey"(ss ‖ kem ‖ cmx)
plaintext = 0x03 ‖ diversifier (11) ‖ value (8) ‖ rseed (32) ‖ memo (512)  the V3 plaintext
enc       = AEAD(key, plaintext)                                          580 bytes
ock       = H256_"ZokaQ_OutCiphKey"(ovk ‖ cv_net ‖ cmx ‖ kem)
out       = AEAD(ock, Orchard raw address (43) ‖ ss (32))                  91 bytes
```

`rseed` is the note's 32-byte seed, `rho` its `rho` (in Orchard, the nullifier of
the note spent in the same action), `cmx` its extracted note commitment.

**Recipient, full decryption** (`try_note_decryption`):

1. `ss = X-Wing.Decaps(xwing_sk, kem)`; a non-contributory X25519 share is
   rejected.
2. Open `enc` with `key`. The lead byte must be `0x03`.
3. For each scope (external, internal): rebuild the address from the diversifier,
   rebuild the note with `rho = nf` of the action, and require that its
   commitment equal the published `cmx`.
4. Recompute `Encaps_derand(own xwing_pk, eseed)` from the rebuilt note and
   require that it equal the published `kem` (the counterpart of Orchard's
   `epk = [esk] g_d` check).

**Recipient, compact decryption** (`try_compact_note_decryption`, light
wallets): steps 1, 3 and 4 over the first 52 bytes of `enc`, decrypted with the
ChaCha20 key stream from block 1 and **without the Poly1305 tag** (as in ZIP 307).
The commitment check and the re-encapsulation authenticate the result.

**Sender recovery** (`try_output_recovery_with_ovk`): open `out` with `ock`, take
the Orchard address and `ss`, derive `key`, open `enc`, check the diversifier and
rebuild the note against `cmx`. The X-Wing ciphertext is not checked on this path
(the sender may keep only the Orchard part).

**Dummy (padding) outputs:** encapsulated to a fresh throwaway X-Wing key; `out`
is random bytes.

**Public outputs:** `ovk = 0^32`. Anyone can open `out`. Used only by the genesis
allocation: `verify_public_output` checks the Orchard address, the value and the
re-encapsulation to the announced X-Wing key.

**Design notes for review:**

- The encapsulation is deterministic in `eseed`, which is derived from the
  note's secret `rseed` (uniform, unique per note). This is what makes step 4
  possible. The `x-wing` crate warns against repeated or public randomness; we
  believe neither applies.
- The note key binds `ss`, `kem` and `cmx`; `ock` binds `ovk`, `cv_net`, `cmx`
  and `kem`. Each key encrypts one message, so the zero nonce is not reused.

## 6. Transactions

### Wire format (`QuanticTransaction::to_bytes`)

| Field | Bytes |
|---|---:|
| Version (`u32`, 1), network id, kind (transfer 0 / coinbase 1), height (`u32`) | 4 + 1 + 1 + 4 |
| Number of actions (`u16`, 1 to 64) | 2 |
| Per action: `cv_net`, `nf`, `rk`, `cmx` | 4 × 32 |
| Per action: `kem`, `enc`, `out` | 1,120 + 580 + 91 |
| Flags (Ironwood encoding), value balance (`i64`), anchor | 1 + 8 + 32 |
| Proof length (`u32`) and Halo 2 proof (2,720 + 2,272 per action) | 4 + 7,264 for two actions |
| Spend-authorization signatures (one per action) and binding signature | 64 per action + 64 |

The height is the block height in a coinbase and the expiry height in a transfer
(0: none). Decoding requires exact lengths, canonical point and scalar encodings,
`rk` not the identity, the exact proof length for the number of actions, and no
trailing bytes. The network id is part of the signed data: a transaction signed
for one network is invalid on another.

### Identifiers

```text
header    = H256_"ZokaQ_TxHeader__"(version ‖ network ‖ kind ‖ height)
compact   = H256_"ZokaQ_ActCompact"(for each action: nf ‖ cmx ‖ kem ‖ enc[0..52])
memos     = H256_"ZokaQ_ActMemos__"(for each action: enc[52..580])
noncomp   = H256_"ZokaQ_ActNonComp"(for each action: cv_net ‖ rk ‖ out)
effects   = H256_"ZokaQ_BundleEff_"(compact ‖ memos ‖ noncomp ‖ flags ‖ value_balance ‖ anchor)
txid      = H256_"ZokaQ_TxId______"(header ‖ effects)
auth      = H256_"ZokaQ_TxAuth____"(proof ‖ spend-auth signatures ‖ binding signature)
```

- Per-action parts have fixed sizes, so the number of actions is determined by
  the lengths.
- Spend-authorization signatures and the binding signature sign `txid` (the
  sighash), which does not include them or the proof: re-randomizing a signature
  does not change the id (ZIP 244 style).
- A block commits to `txid_root` and `auth_root`, each a binary BLAKE2b tree
  (`ZokaQ_MerkleNode`) whose root also commits to the number of leaves; an odd
  node is promoted, not duplicated (`state::merkle_root`).

## 7. Consensus

### Block

A Quantic block of the node (`types::Block`) carries its transactions in
`quantic_transactions`, the coinbase first. Its `state_root` is
`quantic-v1:<txid_root>:<auth_root>:<tree_root>` (hex), where `tree_root` is the
note-commitment-tree root after the block. Its `private_nullifiers` must equal
the sorted, deduplicated nullifiers of its transactions (the node reuses the
spent-nullifier index of Zenith, which handles reorganizations). The node numbers
blocks from 1; the genesis of Quantic is block 1.

### Proof of work

```text
input = "zoka-quantic-pow-v1" ‖ 0x1f ‖ chain_id ‖ 0x1f ‖ prev_hash ‖ 0x1f ‖ merkle_root
        ‖ 0x1f ‖ state_root ‖ 0x1f ‖ height (u64) ‖ timestamp (u64) ‖ nonce (u64)
```

RandomX on the public networks. The string fields are ASCII (hex or the format
above), so `0x1f` is unambiguous. `merkle_root` covers the transaction bytes and
`state_root` the three roots, so the work covers the whole body. The required
target is not taken from the header: the validator computes it (section 9) and
the hash, read as a big-endian number, must not exceed it.

### Rules

- **Coinbase:** first transaction; kind coinbase; height field equal to the
  block height; flags spends-disabled; anchor of the empty tree; at most 64
  actions; value balance exactly `−(minted + fees)`, where `minted` is the
  genesis allocation at the genesis and the subsidy afterwards.
- **Genesis allocation:** the genesis coinbase has exactly one action, which must
  pass `verify_public_output` against the address and amount in the network
  parameters.
- **Transfers:** kind transfer; flags spends and outputs enabled; value balance
  ≥ 0 (it is the fee; nothing else leaves the pool); not expired; at most 64
  actions; no repeated nullifier; anchor equal to the tree root at the end of one
  of the last 100 blocks of the same branch; nullifiers not spent on that branch.
- **Block:** at most 512 transfer actions; no nullifier repeated across the
  block, coinbase included; fees and minted value summed with overflow checks;
  the total minted never exceeds 23,000,000 ZKA (5,000,000 at genesis, then
  12 ZKA for 1,051,200 blocks and 6 ZKA for 897,600).
- **Coinbase maturity by delayed insertion:** the commitments of the coinbase of
  block `h` are appended to the tree at block `h + 60` (2 on regtest), before
  that block's transfer outputs; no anchor contains them earlier, so they cannot
  be spent earlier. The genesis allocation is appended at the genesis.
- **Order of appends at block `h`:** maturing coinbase commitments (from
  `h − 60`), then the genesis coinbase (only at the genesis), then the outputs of
  the transfers in block order.
- **Proofs and signatures** of a block are verified in one `BatchValidator`.
- **Relay policy, not consensus:** conventional fee 0.0001 ZKA per action with a
  minimum of two actions (ZIP 317 style).

## 8. Light wallets

A compact transaction carries the header fields, flags, value balance, anchor,
the `memos` and `noncomp` digests, and for each action `nf ‖ cmx ‖ kem ‖ enc[0..52]`
(1,236 bytes). A light wallet, for each block in order:

1. requires that it be the next height and that `prev_hash` be the hash of the
   last block it scanned; on networks with a fixed genesis, that the genesis
   block have the compiled hash;
2. recomputes every `txid` from the compact data and requires the block's
   `txid_root` to equal the one in `state_root`;
3. scans with compact decryption, appends every `cmx` with the same maturity
   schedule as the node, and requires its tree root to equal the `tree_root` of
   `state_root`.

Phones fetch compact blocks over `/zoka-quantic/light/1` from any peer, through
Tor. The wallet does not verify proof of work (no RandomX on a phone): it trusts
the peers for the header chain, and verifies everything else.

## 9. Difficulty and chain selection

- Target: a 256-bit number in Bitcoin's compact format; a block is valid if its
  proof-of-work hash, big endian, is at most the target.
- ASERT (`aserti3-2d`, integer arithmetic as specified by Bitcoin Cash) anchored
  at the genesis: the target of a block is
  `genesis_target · 2^((t_parent − t_genesis − 120 · (h_parent − h_genesis)) / 3600)`,
  clamped to the floor target (12 leading zero bits). Testnet genesis target:
  14 bits. Regtest: fixed 8 bits.
- Chain selection by accumulated work, each block counting `2^256 / (target + 1)`
  for the target it had to meet.
- Timestamps: greater than the median of the previous 11 blocks of the branch,
  and at most two hours ahead of the validating node's clock.
- The team's signed checkpoints (section 10) override chain selection below the
  latest checkpoint.

## 10. Hybrid signatures

```text
signed = domain ‖ 0x1f ‖ message                 domain: 1 to 255 bytes
sig    = Ed25519.Sign(signed) ‖ ML-DSA-65.Sign(signed, context = domain)
public = 0x01 ‖ Ed25519 public key (32) ‖ ML-DSA-65 public key (1,952)
```

Verification requires both halves; Ed25519 uses strict verification. Used by the
team's chain checkpoints (format version 2) and by bootstrap snapshots. Once a
network's binary has a post-quantum checkpoint key compiled in, only version 2
checkpoints are accepted. A checkpoint decides which valid chain is canonical
below its height; it cannot make an invalid block valid.

## 11. Hypotheses to verify

- **H1 — Recipient privacy against a quantum adversary who knows the address.**
  Given the archived chain and the recipient's address, an adversary who can
  compute discrete logarithms cannot tell whether an output pays that address,
  provided ML-KEM-768 holds. Requires: anonymity (key privacy) of X-Wing
  ciphertexts; that `enc`, `out` and `cmx` reveal nothing without `ss` or `ovk`;
  that the placeholder `epk` carries nothing.
- **H2 — Spend unlinkability against the same adversary.** From `nf`, `rk`,
  `cv_net` and the proof, it cannot link a spend to the output it consumes.
  Requires that `nk` not be derivable from the address (an adversary can obtain
  `ivk` from `pk_d = [ivk] g_d`; we believe `nk` remains hidden behind
  `ivk = Commit^ivk_rivk(ak, nk)`), and that the Halo 2 proof be zero knowledge
  without computational assumptions. ZIP 2005 states the corresponding property
  for Orchard against adversaries without the addresses.
- **H3 — Envelope binding.** A malicious sender cannot produce an action that two
  different wallets accept, or that a wallet accepts with a note other than the
  one committed in `cmx`, or whose `kem` was not derived from the note.
- **H4 — Deterministic encapsulation.** `Encaps_derand` with
  `eseed = H512(rseed ‖ rho ‖ cmx)` is as secure as randomized encapsulation,
  given that `rseed` is uniform and never reused.
- **H5 — Dummy outputs** are indistinguishable from real ones.
- **H6 — The genesis allocation** being a public output does not make its later
  spends linkable.
- **H7 — Address integrity.** No string that a user would accept as the same
  address (same prefix and beginning) can carry a different X-Wing key, and no
  address decodes on two networks.
- **H8 — Transaction ids** commit to every effecting field; nothing a third party
  can change alters a transaction's effects without changing its id; the
  authorization digest and the block's two roots commit to the proofs and
  signatures.
- **H9 — Supply.** The coinbase rules, the delayed insertion and the anchor
  window allow neither spending immature coinbase notes nor minting beyond the
  schedule, including across reorganizations.
- **H10 — Light wallets.** A peer that serves compact blocks cannot make a wallet
  accept a payment that does not exist, miss one that does, or build a transfer
  against a wrong tree, without the checks of section 8 failing; except by
  serving a whole false header chain.
- **H11 — Hybrid signatures.** No combination of a valid signature of one half
  and a forgery of the other verifies; no signature transfers across domains.

## 12. Known limits

- Validity is classical: a quantum adversary could forge proofs and create
  coins, or spend others' notes. The V3 note format keeps a recovery path
  (ZIP 2005); no recovery protocol exists.
- Fees, the number of actions, timing and transaction sizes are public.
- The X-Wing specification is a draft; the post-quantum crates are pre-1.0.
- Light wallets trust their peers for proof of work.
- The team's checkpoint key is a declared trust assumption.

## 13. Running the tests

```sh
cargo test --manifest-path crates/zoka-quantic/Cargo.toml      # 34 unit, 7 end to end (real Halo 2 proofs)
cargo test --features quantic --lib -- quantic:: network::     # the node profile, P2P, light wallets
```

Fixed test vectors (keys and addresses on the three networks, a note envelope
with every input fixed, and a seeded genesis coinbase with its txid and
authorization digest) are in `crates/zoka-quantic/tests/vectors/quantic-v1.txt`,
checked by `cargo test --manifest-path crates/zoka-quantic/Cargo.toml --test vectors`.
Testnet and regtest share `coin_type` 1, so the same root gives the same keys on
both; only the address prefix differs.

The end-to-end tests cover the genesis allocation, payments with change,
coinbase maturity, double spends, the frauds of section 7, light wallets that
detect hidden or altered outputs, saved wallet states, and transactions that stop
verifying when any byte changes.
