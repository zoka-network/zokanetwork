# Proof v4 and network upgrades

Zoka Quantic Testnet v3, November 2026. Proof v4 is **active since block
10,000** (node 5.1.0). Code:
[transfer/narrow.rs](../crates/zoka-pq-pool/src/transfer/narrow.rs); activation
rules in [ledger.rs](../crates/zoka-pq-pool/src/ledger.rs).

## The problem

On testnet v3, a payment that spends 2 notes carried a 2.59 MB STARK proof, and
one that spends 8 notes, 7.85 MB. The v3 circuit copies the whole witness into
columns that stay constant across every row of the trace: about 1,100 columns
per spent note (32 levels × 16 words of Merkle siblings, and as many
intermediate nodes). Each of the 160 FRI queries opens a full row. The
measurements agree: every extra spent note added 0.88 MB.

As a result, the chain grew ~2.6 MB per payment and a 32 MiB block held about
12 payments. On a 32-bit phone an 8-note payment took 90 s and 741 MB of
memory.

## What changed

### Narrow circuit (proof version 4)

v4 runs the same `Program` as v3, with the same jobs, messages, domains,
padding inputs and public values. Both versions prove the same statements, in
far fewer columns:

- Each value lives in a column from the row where a job writes it, or where the
  witness loads it, to the last row that reads it. After that, the column
  carries another value.
- The rows in which a column holds, receives or loads a value are periodic
  columns fixed by the payment shape: the prover chooses nothing.
- The balance adds each note's 16-bit limbs into four running sums (outputs
  subtract) and is checked, with the carries, in a final row.
- Each input's anchor uses the sum of its limbs, kept in a column until the end
  of its Merkle path.
- It has its own transcript domain (`ZOKA QUANTIC transfer proof v4`): a v4
  proof is never valid as v3, nor the other way round.

Notes, the note tree, nullifiers and addresses are unchanged. Only the layout
of the circuit changed.

### Measured on a PC (release build)

| Shape | Size v3 | Size v4 | Prove v3 | Prove v4 | Verify v3 | Verify v4 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2→2 | 2.59 MB | 0.74 MB | 2.09 s | 0.98 s | 167 ms | 126 ms |
| 4→2 | 4.33 MB | 0.74 MB | 3.72 s | 1.67 s | 226 ms | 143 ms |
| 8→2 | 7.85 MB | 0.79 MB | 13.95 s | 5.16 s | 343 ms | 199 ms |

Columns: 452 in all three shapes (v3: 3,086, 5,550 and 10,478).

### Measured on a 32-bit phone

Samsung A13 (SM-A135M, armeabi-v7a, 3.8 GB). The pool's tests were built with
`cargo ndk -t armeabi-v7a -P 26 test --release --lib --no-run` and run from a
temporary folder in `/data/local/tmp` (`what_each_version_costs`). Peak memory
was read from `VmHWM` in `/proc`.

| Shape | Prove v3 | Prove v4 | Verify v3 | Verify v4 |
| --- | ---: | ---: | ---: | ---: |
| 2→2 | 14.06 s | 5.83 s | 553 ms | 375 ms |
| 4→2 | 26.51 s | 10.33 s | 787 ms | 437 ms |
| 8→2 | 90.11 s | 27.01 s | 1,308 ms | 634 ms |

- Sizes match the PC's (2,593,885 / 737,480 bytes for 2→2; 7,853,113 /
  796,449 for 8→2).
- Peak memory: 741 MB with v3 8→2, and **100 MB with v4 8→2**
  (`the_largest_shape_with_carries_in_every_limb_proves_and_verifies`, 44 s
  including verification).

What a 2→2 v4 proof (739,903 bytes) is made of:

| Part | Bytes |
| --- | ---: |
| Opened trace rows (460 values per query) | 294,882 |
| FRI (10 rounds) | 174,160 |
| Quotient (128 values per query) and its Merkle paths | 155,476 |
| Merkle paths of the trace and masks | 70,128 |
| Out-of-domain and mask openings | 34,091 |
| Mask rows | 10,562 |

## Decisions

1. **The narrow v4 circuit.** It is the big lever, and it changes no notes,
   tree, nullifiers or addresses.
2. **FRI as in v3:** blowup 8, 160 queries, folding by 2, no grinding.
   Plonky3's soundness estimator (`p3-security`) gives v4 the same result as v3
   in all three shapes; the bound is still set by the collision-resistance
   assumption on Keccak. This is an estimate, not an audit. The other settings
   measured earlier saved at most 14 % of the size (blowup 16 with 128 queries)
   and doubled the proving time.
3. **Keccak for the FRI Merkle trees and the transcript**, as in v3. Poseidon2
   stays inside the circuit (notes, nullifiers, the note tree).
4. **Postcard encoding, unchanged.** A fixed-width encoding would save ~15 %
   (field values take ~4.9 bytes instead of 4), but it adds custom code to the
   consensus decoding path. It remains a possible future version.
5. **No aggregation at mainnet launch.** See below.
6. **Post-quantum margin unchanged.** The review of the proof's post-quantum
   soundness is on the 2028 roadmap. A larger challenge field would be another
   proof version, activated by height.

## Aggregation: why not now

To aggregate a block's payments, the miner has to verify each proof inside
another proof. The cost is in re-hashing what each proof opens:

- Per v4 proof: ~97,000 field values in the opened leaves, ~3,700 compressions
  in the Merkle paths and ~3,000 more in FRI. That is 10,000 to 15,000
  Keccak-f permutations per proof, or 120,000 to 180,000 per block of 12
  payments.
- Plonky3's Keccak AIR uses 24 rows and about 2,600 columns per permutation:
  billions of cells per block, against the 460,000 of one v4 proof (1 to 5 s).
  It does not fit on an ordinary PC every 2 minutes.
- With Poseidon2 in the inner proof's Merkle trees (one row of ~300 columns per
  permutation), it would be 60 to 90 million cells: several minutes per block
  on a desktop PC. Possible for a dedicated prover, not for an ordinary miner.

So the mainnet launches with one proof per payment (v4). To save disk, a node
could drop old proofs it has already validated, with archive nodes keeping all
of them. Aggregation remains a future upgrade (a new proof version) if Plonky3's
recursion matures.

## Network upgrades instead of restarts

Zoka does not restart the chain for each change, and the mainnet will never
restart. Format changes arrive as upgrades at an announced height, as Zcash
(Sapling, Orchard) and Monero (RingCT, Bulletproofs, CLSAG) did.

The consensus rule (`ledger::proof_version_allowed`):

- a payment in block `h` may carry proof v4 if `h ≥ H`;
- v3 stays valid up to `H + 100`, a grace period for payments built just
  before;
- `H` is 10,000 on the testnet and 8 on regtest, and every new network (the
  mainnet) has only v4 from genesis.

Wallets prove with `ledger::proof_version_for(network, next block)`: nobody
picks the version. The mempool re-validates with every block, so a v3 payment
left outside the grace period drops out by itself.

Notes, the tree, nullifiers and addresses do not change: balances and wallets
stay the same after the activation.

## The activation on the testnet

- Block 10,000 passed with no split. At height 10,638, three nodes (the team's
  PC and both servers) shared the same tip and kept signing checkpoints, and
  the seeds had rejected no blocks.
- **First v4 payment from the desktop wallet** (ZSilent 5.1.0): block 10,642,
  proof of **745,595 bytes**. The last v3 payment, in block 9,867, carried
  2,593,198.
- **First v4 payment proved on a phone** (ZKAPriv 4.0.1, 32-bit Android):
  block 10,645, proof of **737,051 bytes**.
- Both paid the standard fee of a 2-note payment (0.0002 ZKA), and all three
  nodes stayed on the same chain.
