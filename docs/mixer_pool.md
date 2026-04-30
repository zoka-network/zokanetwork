# Plan: Mixer / pool de anonimato transaccional

## Estado actual

- Tx privadas ya tienen ring signatures (`PRIVATE_TX_MAX_INPUTS = 8`,
  `ring_members` + `ring_signature`) que proveen k-anonymity del emisor
  **dentro de una tx**.
- Dandelion++ ya separa propagación en stem/fluff y agrega padding.
- Cada tx privada viaja sola por el gossip: un observador puede
  cronometrar arrivals y correlacionar por timing, aun con ring
  signatures.

## Goal

Agrupar N tx privadas en un mismo payload de gossip ("batch") de modo
que un observador de red vea un único evento `N_txs_arrive_at_t` en vez
de `N_txs_at_t, t+δ, t+2δ, ...`. Esto no cambia el anonymity set de la
tx individual (el ring signature ya lo da), pero reduce la correlación
de timing entre inputs y outputs en el mempool.

## Design sketch

### Componente: `MixerBatcher`

- Vive en `src/network/p2p.rs` como sidecar de `GossipHandler`.
- Cola FIFO con tiempo de retención `mixer_hold_ms` (default 2000).
- Cuando el hold expira O cuando la cola alcanza `mixer_max_batch`
  (default 8), emite un `MixerBatchMessage { txs: Vec<PrivateTransactionMessage> }`.
- El hold timer se resetea cada vez que el batch se vacía.

### Wire message

```rust
pub struct MixerBatchMessage {
    pub batch_id: String,        // hash de todos los tx_hashes
    pub txs: Vec<PrivateTransactionMessage>,
    pub padding: Vec<u8>,        // relleno uniforme: batch total size fijo
    pub timestamp: u64,
}
```

- Envuelto en `ZokaMessage::PrivateMixerBatch(MixerBatchMessage)`.
- En recepción: `handler.rs` desdobla, cada tx entra individualmente a
  `PrivateTransactionPool`.

### Comportamiento del nodo receptor

- Acepta batch solo si ningún `tx_hash` ya existe en el pool (evita
  replay de batches antiguos).
- Verifica cada tx con la pipeline normal de `validate_inbound_private_transaction`.
- Rechaza el batch entero si cualquier tx falla — esto previene que
  un proposer malicioso "envenene" un batch válido con una tx
  inválida al final.

### Timing jitter en stem→fluff

Complemento al batching:

- Actualmente el stem hop usa `stem_hops_remaining` y transiciona a
  fluff al llegar a cero.
- Propuesta: además del hop count, cada nodo en el stem aplica un
  `stem_delay_ms` aleatorio `U(0, mixer_hold_ms)` antes de reenviar.
- Esto desincroniza el `batch_id` observable: el mismo batch
  originado en t ahora llega a diferentes fluff-layers en t+jitter.

## Cosas que rompen / no rompen

- **No rompe** criptografía. Ninguna CRS re-setup.
- **No rompe** consenso. El validator solo ve las tx individuales
  tras el unbundle.
- **Rompe** wire format: nuevo tipo `ZokaMessage::PrivateMixerBatch`.
  Upgrade coordinado: versioned gossip topic `zoka/mixer/v1`.
- **Impacta** latencia: hasta +2s de delay en tx privadas (configurable).

## Riesgos

- **Reducción real de anonimato**: con ring signatures de tamaño 16
  (defaults típicos), el anonymity set individual ya es grande. Batching
  añade marginal timing-un-correlatability. Medir antes de prometer
  beneficio.
- **DoS por batch flood**: un attacker podría generar batches falsos
  gigantes. Mitigación: límite estricto `mixer_max_batch = 16` +
  rate-limit por peer ID.
- **Partial reveal**: si el padding es insuficiente, el tamaño del batch
  correlaciona con el número de tx. Mitigación: padding a múltiplos
  de 2KB.
- **Determinismo del batch_id**: si el batch_id depende del orden de
  las tx, un proposer puede fingerprint. Mitigación: sort tx por
  `tx_hash` antes de computar `batch_id`.

## Acceptance criteria

- [ ] `MixerBatcher` implementado con cola + timer.
- [ ] `MixerBatchMessage` serializa/deserializa correctamente.
- [ ] Integration test: 10 tx privadas emitidas en ráfaga llegan en
  ≤3 batches.
- [ ] Property test: batch que contiene una tx inválida es rechazado
  íntegro.
- [ ] Rate-limit por peer activo.

## Estimación

1 semana (1 ingeniero). Es trabajo contenido a `network/`. Ganancia de
privacidad marginal frente a lo que ya dan ring signatures; priorizar
solo si el anonymity set de rings es insuficiente para el threat model
objetivo.
