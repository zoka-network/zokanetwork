# Mainnet 1.9.0 Recovery Runbook

This runbook coordinates the recovery from the forks observed around mainnet
height 10887. It is deliberately non-destructive: never delete, reset, or
overwrite a node database as part of the automatic recovery path.

## Frozen recovery parameters

- Protocol: `/zoka/1.9.0-mainnet-recovery`
- Minimum client: `1.9.0`
- Checkpoint height: `10887`
- Checkpoint hash:
  `d30c20100f6405424d32e5c204799bc10a1197ff5cdc1fe0b04c2c538b11086e`
- Minimum confirmation head before publication: `11007` (120 blocks above the
  checkpoint)
- Independent observations: at least two, consisting of the canonical seed and
  at least one separate peer reporting the same height/hash
- Release artifact: SHA-256 checksum required
- Signed snapshot: recommended when available, but not a publication gate;
  imported blocks are revalidated by consensus

The checked-in manifest is a blocked release candidate. Do not publish or roll
out 1.9.0 while `recovery.release_gate.allow_publish` is `false`.

## Evidence collection

Collect evidence from each source without copying wallet material, IP-derived
secrets, authentication tokens, or private keys. Record:

1. observation timestamp in UTC;
2. peer ID and operator/source label;
3. reported head height and hash;
4. block 10887 hash;
5. the command/tool version used to obtain the observation.

The two sources must be operationally independent. Two endpoints backed by the
same database count as one observation. Both must report the frozen checkpoint,
and the canonical head must be at least 11007 before the release gate can move
to approved.

Build the final binaries reproducibly, calculate SHA-256 for every published
artifact, and have a second operator verify the checksum. Record that checksum
in the manifest in the same reviewed change that sets `allow_publish` to true.
That change must also populate the two observations and update the release-gate
tests; it must not change the compiled checkpoint.

## Node behaviour during recovery

1. A node below 10887 continues normal P2P sync.
2. A node at or above 10887 whose canonical height index has a different hash
   enters checkpoint quarantine. Sync and block ingestion remain active, but
   mining (including local runtime mining control) is disabled.
3. A side branch is stored and fully validated block by block. It cannot be
   promoted merely because a peer announces a height or hash.
4. Once that validated branch proves the compiled 10887 checkpoint, it may be
   promoted even when the invalid local fork reports more cumulative work. The
   canonical index, head metadata, and nullifier index switch in one RocksDB
   batch. The old branch bodies remain on disk for audit and recovery.
5. A branch that does not prove the checkpoint is never force-promoted.

This is a narrow 1.9.0 consensus migration, not a general permission to prefer
lower-work branches. Tor, TCP, and iroh only transport data; transport health
does not establish which fork is canonical.

## Staged rollout

1. Stop mining on candidate nodes and make an offline copy of each database.
   Record the source path, time, head height/hash, and backup checksum.
2. Confirm the publication evidence above. If any observation disagrees, keep
   the gate blocked and investigate; do not choose a new checkpoint ad hoc.
3. Start one 1.9.0 canary with mining disabled. Confirm it connects only to the
   1.9.0 protocol, retains P2P sync while quarantined, and converges to the
   checkpoint branch.
4. Verify the canary's block 10887 hash and current head against both evidence
   sources. Confirm the old fork block bodies still exist.
5. Upgrade the remaining verifier nodes with mining disabled, then miners one
   at a time. Enable mining only after the node's checkpoint-history check is
   clean and it is caught up to the agreed head.
6. Monitor peer count, checkpoint quarantine state, sync errors, canonical
   height/hash, and mining state throughout the rollout.

## Failure and manual recovery

If a node cannot assemble the validated checkpoint branch (for example because
required ancestors are unavailable), leave it quarantined and preserve its
database. Do not script deletion or an in-place reset.

An operator may instead create a new data directory and import a separately
verified snapshot or perform a fresh block-by-block sync. Keep the original
directory read-only until balances, nullifiers, block 10887, and the new head
have been audited. A signed snapshot is preferable when available, but its
blocks must still pass normal consensus validation.

If the 1.9.0 binary itself is faulty, stop mining and retain the database. Roll
back the executable only under a coordinated operator decision; an older client
must not resume mining on the incompatible pre-recovery protocol. Never use a
release rollback as authorization to delete or rewrite chain data.
