# Nodo, minero y ZSilent 5.1.0, ZKAPriv 4.0.1: prueba v4 (actualización de red)

**Publicado el 25/11/2026.** Primera actualización de red de la testnet v3: la
cadena sigue, no se reinicia. Detalle técnico en [PRUEBA-V4.md](PRUEBA-V4.md).

## Qué trae

- **Billetera de la PC sobre el motor liviano** (el de ZKAPriv y la web): el
  saldo y el historial leen solo los bloques nuevos. 0,2 s por consulta en vez
  de ~30 s; la primera vez lee la cadena entera una sola vez (~20 s). La carpeta
  `wallet-mirror` (1 GB) deja de usarse.
- **ZSilent sin cola:** una sola lectura de saldo a la vez y solo cuando entra
  un bloque; el envío pasa primero.
- **Prueba de pago v4** (circuito angosto): 0,74 MB en vez de 2,59 MB (2 notas)
  y 0,80 MB en vez de 7,85 MB (8 notas). En el teléfono de 32 bits, un pago de
  8 notas tarda 27 s en vez de 90 s y usa 100 MB de memoria en vez de 741 MB.
  Mismas notas, mismo árbol, mismas direcciones y la misma seguridad estimada.
- **El minero CLI baja XMRig solo:** `2-get-xmrig.cmd` / `./zoka-miner.sh
  get-xmrig` trae XMRig 6.25.0 de sus releases oficiales y controla el
  SHA-256 de su `SHA256SUMS`; el de minar pasó a `3-start-mining.cmd`. La guía
  explica cómo permitir XMRig en Defender y cómo deshacerlo. XMRig sigue sin
  venir adentro: los antivirus marcarían el paquete entero. Se agregó el mismo
  día, después de publicar, y reemplazó el zip y el tar.gz de la release
  (antes `a15c25b5…` y `394d6292…`; apartados en
  `target/cli-miner/5.1.0-sin-get-xmrig`).
- **Activación en el bloque 10.000 de la testnet.** Desde ahí, los pagos llevan
  la prueba v4; los de la v3 valen hasta el 10.099. Las billeteras eligen la
  versión solas.

## La altura

Estaba en 20.000 (~9/12). El usuario la pasó a **10.000** el 25/11 a la altura
9.865, para ver la activación esa misma tarde, y se recompiló todo
(`58feddaa`). La PC era la única que minaba: con ZSilent cerrada para
instalar, la cadena quedó quieta en el 9.866 desde las 12:35. **El 10.000
llega cuando vuelva a minar.** ASERT baja la dificultad mientras la cadena
está atrasada, así que los primeros bloques salen más rápido.

## Publicado

| Qué | Versión | Archivo / hash | Commit |
| --- | --- | --- | --- |
| Nodo y minero CLI (Latest; 5.0.1 Superseded) | 5.1.0 | zip `48a6d404…`, tar.gz `362198cf…`, Linux `a18e3dbb…`; `zoka.exe` `27c0abc3…`, `zokahd.exe` `005d0e4c…` | nodo `58feddaa` = etiqueta `v5.1.0`; scripts del paquete `ec1cd9c8` |
| ZSilent (Latest; 5.0.1 Superseded) | 5.1.0 | `ZSilent-Core-5.1.0.msi` `81789fb1…` (por dentro: zoka, zokahd y Tor 0.4.9.14) | `5e0ee8b` = etiqueta `v5.1.0` |
| ZKAPriv (Latest; 4.0.0 Superseded) | 4.0.1 (código 21) | `ZKAPriv-4.0.1.apk` `2020c649…`, firma `061376f8…` | `872f285` = etiqueta `v4.0.1` |
| Billetera web | WASM `c742cc80…` | wallet.zoka.network (Pages `zokawallet`) | zokawallet `68a5b20` |
| zoka.network | descargas 5.1.0 / 4.0.1, mapa | Pages `zokaweb` | zokaweb `1a2c245` |
| VPS 1 y 2 (semillas, autoridades, faucet, API del explorador) | 5.1.0 | `scripts\vps-actualizar-binario.ps1`; la 5.0.1 quedó en `/usr/local/bin/zoka-5.0.1` | — |
| PC | ZSilent 5.1.0 | instalada por el usuario; nodo autoridad, conectado a las dos semillas | — |

`main` y etiquetas en GitHub y GitLab en los cinco repos. Archivos en
`target/release-5.1.0/{node,zsilent,zkapriv}`, cada carpeta con su
`SHA256SUMS.txt`. Los armados con la altura 20.000 quedaron apartados como
`…-descartado-altura-20000` (no se publicaron).

## Verificación

- `zoka-pq-pool` con la altura 10.000: 51 pruebas sin fallas (3 ignoradas de
  medición). Antes, con la misma lógica: el nodo entero (`quantic::`, 104), con
  el ensayo de la activación en regtest, y la billetera web (`npm test`, 38).
- El MSI abierto por extracción administrativa: lleva exactamente los
  `zoka.exe` y `zokahd.exe` nuevos y el Tor verificado.
- Las tres releases bajadas de vuelta de GitHub: `sha256sum -c` OK en todos los
  archivos; el manifiesto de ZSilent, idéntico.
- La billetera web y zoka.network, controladas en vivo (el WASM y las versiones
  coinciden con lo compilado).
- Teléfono (Samsung A13, 32 bits): la 4.0.1 se instaló encima de la 4.0.0,
  abrió, se desbloqueó y sincroniza («sync ok via https» cada ~12 s, sin
  errores). La v4 medida ahí: ver [PRUEBA-V4.md](PRUEBA-V4.md).

**GitHub tuvo un incidente propio** (errores 500 al crear releases): se
crearon como borrador, se completaron los adjuntos y se publicaron aparte. Ojo:
`gh release view <etiqueta>` también encuentra borradores.

## La activación (26/11)

- El bloque 10.000 pasó sin cortes. El 26/11 a la altura 10.638, la PC, el
  VPS 1 y el VPS 2 estaban en la misma punta, firmando checkpoints, sin
  bloques rechazados en las semillas. El faucet ya funciona con el motor
  liviano (`faucet.json.light`) y dio por cerrado solo su pago pendiente viejo.
- **Primer pago con la prueba v4:** tx `0f8a8e7c…` (ZSilent 5.1.0, 1 ZKA),
  minada en el bloque **10.642** con una prueba de **745.595 bytes** (el
  último pago v3, en el 9.867, llevaba 2.593.198). Comisión 0,0002 ZKA (2→2).
  Los tres nodos tienen el mismo 10.642 (`644f6a99…`) y siguen juntos.

## Lo que queda

1. ~~Volver a minar y pasar el 10.000~~: hecho.
2. **Discord (usuario):** `discord-bot/notice.upgrade-5.1.0.json` (id
   `upgrade-5-1-0`). Vista previa con
   `node scripts/send-notice.js notice.upgrade-5.1.0.json`; para mandarlo,
   `"draft": false` en el JSON y
   `$env:DISCORD_NOTICE_CONFIRM = "upgrade-5-1-0"; node scripts/send-notice.js notice.upgrade-5.1.0.json --send`.
3. **Después del 10.000:** un pago de prueba (lo hace el usuario) y controlar
   por la API que lleve la prueba v4, que se mine y que los VPS y el explorador
   lo sigan.
4. Opcional: renombrar `wallet-mirror` en la PC y en el faucet (no se usa más).
