# Nodo, minero y ZSilent 5.1.0: envíos rápidos y prueba v4 (actualización de red)

Preparación del 25/11/2026. **Primera actualización de red de la testnet v3: la
cadena sigue, no se reinicia.** Detalle técnico en [PRUEBA-V4.md](PRUEBA-V4.md).

## Qué trae

- **Billetera de la PC sobre el motor liviano** (el de ZKAPriv y la web): el
  saldo y el historial leen solo los bloques nuevos. 0,2 s por consulta en vez
  de ~30 s; la primera vez lee la cadena entera una sola vez (~20 s). La carpeta
  `wallet-mirror` (1 GB) deja de usarse y se puede borrar.
- **ZSilent sin cola:** una sola lectura de saldo a la vez y solo cuando entra
  un bloque; el envío pasa primero. Un envío dejó de esperar minutos.
- **Prueba de pago v4** (circuito angosto): 0,74 MB en vez de 2,59 MB (2 notas)
  y 0,79 MB en vez de 7,85 MB (8 notas); probar 2-3 veces más rápido y
  verificar un 25-40 % más rápido. Mismas notas, mismo árbol, mismas
  direcciones y la misma seguridad estimada (128 bits clásicos).
- **Activación en el bloque 20.000 de la testnet:** desde ahí los pagos llevan
  la prueba v4; los de la v3 valen hasta el 20.099. Las billeteras eligen la
  versión solas.

## Antes del bloque 20.000 tienen que actualizar

- Las tres autoridades de checkpoints (PC, VPS 1 y VPS 2) y las semillas.
- El faucet (VPS 1).
- Mineros y usuarios de ZSilent, ZKAPriv y la billetera web.

Un nodo que no actualice rechaza los bloques con pagos v4 y queda en otra rama.

## Verificación

- [x] `zoka-pq-pool`: todas las pruebas, más las 7 del circuito v4 y la de la
  regla de activación.
- [x] `quantic-pq-wallet`: punta a punta contra la API HTTP de un nodo de
  regtest (saldo, historial, pago reservado, minado y recibido, estado dañado).
- [ ] Todo `quantic::` del nodo, con el ensayo de la activación en regtest.
- [ ] Binarios 5.1.0 de Windows y Linux, MSI de ZSilent, zips del minero.
- [ ] `.so` y APK de ZKAPriv 4.0.1; WASM de la billetera web.
- [ ] La v4 medida en el teléfono de 32 bits.
- [ ] Publicar, instalar en la PC y en los VPS, faucet, Discord (usuario).

## Notas para GitHub (inglés)

```text
Zoka Quantic 5.1.0 — fast wallet and proof v4 (network upgrade at block 20,000)

This is the first network upgrade of testnet v3: the chain continues, nothing
restarts, balances and addresses stay as they are.

- Wallet on the light engine: balance and history read only new blocks
  (0.2 s instead of ~30 s). The wallet-mirror folder is no longer used.
- ZSilent no longer queues balance reads; a send goes first.
- Payment proof v4: 0.74 MB instead of 2.59 MB (2 notes), 0.79 MB instead of
  7.85 MB (8 notes); 2-3x faster to prove.
- From block 20,000 payments carry proof v4 (v3 stays valid up to 20,099).
  Wallets pick the version by themselves.

Upgrade your node, miner and wallets before block 20,000: an old node rejects
blocks with v4 payments.
```
