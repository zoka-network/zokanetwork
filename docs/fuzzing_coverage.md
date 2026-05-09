# Plan: Fuzzing + property testing extendido

## Estado actual

Property tests existentes (`cargo test --lib`, ~5 casos por prop):

- `prop_private_transaction_roundtrip_and_validation` â€” tx privada
  construida por alice, validada por el manager, descifrada por bob.
- `prop_tampered_ring_id_is_rejected` â€” mutaciÃ³n de `ring_id` rechazada.
- `prop_verifier_accepts_valid_private_tx_proofs` â€” el `Verifier` acepta
  proofs Groth16 generadas honestamente.
- `prop_private_fee_produces_valid_range_proof` (nuevo) â€” fee > 0
  emite Bulletproof verificable.
- `prop_tampered_fee_range_proof_is_rejected` (nuevo) â€” byte flip en
  fee range proof â†’ rechazo.

Superficie **sin** cobertura sistemÃ¡tica:

- DeserializaciÃ³n adversarial de `PrivateTransaction` / `Block` /
  `BlockHeader` desde JSON arbitrario.
- `verify_output_range` con range proofs malformados (vs solo byte
  flips).
- `verify_directory_digest` con tampering de archivos en disco.
- Circuitos ZK con **witness** cerca de bounds (2^32-1, 0, valores
  donde u32 wraparound podrÃ­a dispararse).
- Ring signatures con `ring_members` que contienen puntos no-Ristretto
  (punto invÃ¡lido + firma clonada).

## Goal

Llevar el proyecto a un estado donde:
1. `cargo +nightly fuzz` corre â‰¥4 harnesses con â‰¥1M executions sin
   crash, panic, ni hang.
2. Property tests cubren cada edge case conocido del modelo de amenaza
   en comentarios del cÃ³digo.
3. CI corre un subset de property tests (ej. 16 casos por prop) en
   cada PR.

## Harnesses propuestos (`fuzz/fuzz_targets/`)

### `fuzz_private_tx_deserialize.rs`
```rust
fuzz_target!(|data: &[u8]| {
    if let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(data) {
        // DeserializaciÃ³n ok: ni siquiera el validador debe panicar.
        let _ = validate_inbound_private_transaction(&tx);
    }
});
```

**Atrapa:** overflow en longitudes, unchecked unwraps, panics en
`point_from_hex`.

### `fuzz_range_proof_verify.rs`
```rust
fuzz_target!(|(commit, proof): (String, Vec<u8>)| {
    let _ = verify_output_range(&commit, &proof);
});
```

**Atrapa:** panics en bulletproof parser, OOM por proofs gigantes
no validados.

### `fuzz_block_merkle.rs`
```rust
fuzz_target!(|block: Block| {
    let _ = block.compute_merkle_root();
    let _ = block.compute_hash();
});
```

**Atrapa:** overflow en merkle root cuando `transactions.len()` +
`private_tx_hashes.len()` cruzan lÃ­mites raros.

### `fuzz_verifier_groth16.rs`
```rust
fuzz_target!(|(circuit_tag, proof_bytes, pub_inputs): (String, Vec<u8>, Vec<Vec<u8>>)| {
    let proof = ZKProof::new(proof_bytes, circuit_tag, pub_inputs, 0);
    let verifier = Verifier::new();
    let _ = verifier.verify(&proof);
});
```

**Atrapa:** panics en `Proof::<Bls12>::read`, manejo de public inputs
malformados.

## Property tests adicionales sugeridos

AÃ±adir a `src/private_wallet/mod.rs` y a `src/zk/verifier.rs`:

- **prop_balance_wraparound_rejected**: fuerza `input_sum` cerca de
  `u32::MAX`, verifica que el circuito rechaza.
- **prop_swap_fee_and_amount_commitment**: intercambiar
  `fee_commitment` con un output `amount_commitment` rompe la validaciÃ³n.
- **prop_duplicate_nullifier_across_tx**: dos tx con mismo nullifier
  â†’ segunda rechazada.
- **prop_invalid_ring_member_point**: un miembro del ring no es
  Ristretto vÃ¡lido â†’ rechazado.

## Infraestructura

- AÃ±adir `cargo-fuzz = "0.11"` como tool (no dep).
- Crear `fuzz/Cargo.toml` con los harnesses.
- Documentar en README: `cargo +nightly fuzz run fuzz_private_tx_deserialize`.
- CI: GitHub Action que corre cada harness 5 min en PRs, 1 hora en
  nightly.

## Riesgos / limitaciones

- **Fuzzing solo en nightly**: `cargo-fuzz` requiere rust nightly.
  Documentar. No bloquea CI de PR.
- **Corpus inicial vacÃ­o**: primeros runs encuentran basura. Seed con
  tx vÃ¡lidas serializadas + mutaciones manuales.
- **Proptest slow**: proofs Groth16 son caros (~500ms cada uno). Subir
  `ProptestConfig::with_cases(16)` a 64 solo en runs de nightly.

## Acceptance criteria

- [x] 4 fuzz harnesses en `fuzz/fuzz_targets/`.
- [ ] 4 property tests nuevos en `private_wallet` y `zk::verifier`.
- [ ] `cargo +nightly fuzz run <harness>` corre 1M executions sin
  incidentes en cada harness.
- [ ] GitHub Action configurada.
- [ ] README con ejemplo de uso.

## EstimaciÃ³n

1 semana para harnesses + infra. +2â€“3 dÃ­as de seeding manual por
harness. Corre en paralelo con cualquier otra pieza â€” no bloquea.
