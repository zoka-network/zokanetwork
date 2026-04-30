# Plan: Confidential multi-asset (#7)

## Estado actual

ZOKA maneja un único activo. Todos los `amount_commitment` comprometen
la misma generator point `G` para el valor. No hay concepto de "asset
tag" o "asset ID".

## Goal

Soportar múltiples activos (p.ej., distintos stablecoins, NFTs,
tokens fungibles) donde:

- El tipo de activo de cada input/output está **comprometido pero
  oculto** (confidential assets à la Zcash Sapling / Elements).
- La suma de cada activo se preserva independientemente: tx válida
  implica `sum(inputs_A) = sum(outputs_A) + fee_A` para cada asset A.
- Un observador no puede determinar **qué** activo se está moviendo,
  solo que algún gasto válido ocurrió.

## Design sketch

### Asset tag commitments

Cada commitment de monto se expande de Pedersen 2-generator a
3-generator:

```
amount_commitment = amount · G_asset + blind · H
```

donde `G_asset = H_to_point(asset_id)` es un generator determinístico
por-asset. El valor `amount` queda oculto **y** el tipo de asset queda
oculto detrás de `G_asset`.

### Circuito

`GrothPrivateTxCircuit` añade:

- **Public input**: nada (el asset tag NO se revela).
- **Witness**: `asset_tag_scalar` por cada input y output.
- **Constraint**: cada output consume el mismo `asset_tag_scalar` que
  su input fuente (via un selector que el circuito verifica). Los
  generators `G_asset = H_to_point(asset_tag_scalar)` computados
  dentro del circuito — expensive.

Alternativa más simple: `PRIVATE_TX_MAX_ASSETS = 4`, el circuito
soporta hasta 4 activos simultáneos por tx, con balance check por
slot. Esto limita el caso de uso (tx atómicas multi-asset) pero
mantiene el proof tamaño-fijo.

### Wire format

- `PrivateInput.asset_tag_commitment: String` (Ristretto point).
- `PrivateOutput.asset_tag_commitment: String`.
- Ambos incluidos en el tx_hash para evitar malleability.

## Breaking changes

- **CRS**: nuevas public inputs y witnesses → regenerar CRS → rehacer
  ceremonia.
- **Wire format**: tx privadas antiguas ya no validan. Requiere hard
  fork o versioning.
- **Wallet**: el modelo `PrivateNote` necesita un `asset_id` adicional.
  Wallet storage rompe compatibilidad.

## Dependencias

- `#5 Ceremonia MPC` debe rehacerse **después** de este cambio.
- `#3 Fees privados` necesita fee por asset → fee_commitment por asset.

## Riesgos

- **Explosión de constraints**: `hash_to_point` dentro del circuito es
  muy caro (cientos de constraints por asset). Proving time puede
  subir 5–10x.
- **Ataque de censura por asset**: si el asset tag se filtra de algún
  modo (side-channel, pattern de bulletproofs), un validador puede
  rechazar por asset. Monero-style confidential assets usa blinding
  factors por-asset adicionales para prevenir esto.
- **UX**: crear wallets multi-asset requiere rethink de balance
  queries, notes por asset, coin selection.

## Recomendación

**Postergar hasta después de que ZOKA tenga una aplicación concreta que
pida multi-asset.** Es scope creep para un blockchain genérico;
justificación sólida solo si hay clientes que necesitan intercambio
atómico de múltiples activos on-chain.

Si se implementa, hacerlo **junto** con `#5 Ceremonia MPC` final —
no rehacer ceremonia dos veces.

## Acceptance criteria (si se implementa)

- [ ] Hasta 4 assets distintos en una tx.
- [ ] Balance equation verificada por asset dentro del circuito.
- [ ] Proof size ≤ 2x el actual.
- [ ] Proving time ≤ 3x el actual.
- [ ] Test: tx con 2 assets distintos verifica OK.
- [ ] Test: balance violation en un asset → rechazo.

## Estimación

2–3 semanas si `hash_to_point`-en-circuito se puede cachear; 4–6
semanas si requiere reescritura de gadgets.
